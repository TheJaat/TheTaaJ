/* format.c - what printf does: a format and some values in, text out.
 *
 * This is the part of the C library written in C, and the reason is
 * the three dots. `printf(const char *format, ...)` takes any number
 * of arguments of any types, and only the format string says what they
 * are. C has a way to walk such arguments (va_list, va_arg); it is the
 * natural language for the one function that needs it most.
 *
 * Nothing here knows where the text goes. __taaj_format hands each
 * piece to a function it is given: one that fills a buffer (snprintf,
 * below), or one that writes to a descriptor (printf, in stdio.c). And
 * nothing here asks the system for anything - so this file can be
 * compiled for the machine it is written on and compared, format by
 * format, with that machine's own printf: `make libc-test`.
 *
 * What a conversion looks like:
 *
 *     %  [flags]  [width]  [.precision]  [length]  conversion
 *        - 0 + space #     5 or *        .3 or .*   hh h l ll z j t L
 *
 * Conversions: d i u x X o c s p %, and the floating-point ones f F e E
 * g G. Not here: a A (hexadecimal floating point), and n. A conversion
 * that is not known is printed as it was written.
 *
 * A long double (%Lf) is printed as the double nearest to it: on the PC
 * that loses the x87's eleven extra bits. */

#include "format.h"

#include <stdint.h>
#include <string.h>

/* Where the text is going, and how much has gone. */
struct output {
    emit_fn emit;
    void *context;
    size_t total;
};

static void put(struct output *out, const char *bytes, size_t count)
{
    if (count > 0)
        out->emit(out->context, bytes, count);
    out->total += count;
}

/* The same byte, count times: the spaces and zeros of padding. */
static void fill(struct output *out, char byte, size_t count)
{
    char run[16];
    memset(run, byte, sizeof run);
    while (count > 0) {
        size_t now = count < sizeof run ? count : sizeof run;
        put(out, run, now);
        count -= now;
    }
}

/* What was written between the % and the conversion letter. */
struct spec {
    int left;       /* -   against the left edge of the width */
    int zero;       /* 0   padded with zeros, not spaces */
    int plus;       /* +   a sign even when positive */
    int space;      /* ' ' a space where the sign would be */
    int alternate;  /* #   0x before hex, 0 before octal */
    int width;      /* the least room to take; 0 if none given */
    int precision;  /* -1 if none given */
};

/* The lengths: how big the argument is. */
enum length { PLAIN, CHAR, SHORT, LONG, LONG_LONG, SIZE, MAX, DIFFERENCE, LONG_DOUBLE };

/* A number written without `*`: the digits at *format, moved past. */
static int number(const char **format)
{
    int value = 0;
    while (**format >= '0' && **format <= '9') {
        value = value * 10 + (**format - '0');
        (*format)++;
    }
    return value;
}

/* A piece of text in a width: spaces before it, or after with `-`. */
static void put_padded(struct output *out, const struct spec *spec, const char *bytes, size_t count)
{
    size_t padding = (size_t)spec->width > count ? (size_t)spec->width - count : 0;
    if (!spec->left)
        fill(out, ' ', padding);
    put(out, bytes, count);
    if (spec->left)
        fill(out, ' ', padding);
}

/* A whole number. `negative` is given apart from the value so that the
 * most negative number - which has no positive twin - needs no special
 * case: the value is always its size, unsigned.
 *
 * The pieces, in the order they are written:
 *
 *     [spaces] [sign] [0x] [zeros] digits [spaces]
 */
static void put_number(struct output *out, const struct spec *spec, uintmax_t value, int negative, int is_signed,
                       unsigned base, int capitals)
{
    /* The digits, last first, at the end of the buffer. 64 bits in
     * octal is 22 digits. */
    char digits[24];
    size_t at = sizeof digits;
    const char *figures = capitals ? "0123456789ABCDEF" : "0123456789abcdef";
    uintmax_t rest = value;
    while (rest > 0) {
        digits[--at] = figures[rest % base];
        rest /= base;
    }
    size_t count = sizeof digits - at;

    /* Precision is the least number of digits; without one it is 1, so
     * that zero is "0". With `.0`, zero is no digits at all. */
    size_t least = spec->precision < 0 ? 1 : (size_t)spec->precision;
    size_t zeros = least > count ? least - count : 0;

    const char *sign = "";
    if (negative)
        sign = "-";
    else if (is_signed && spec->plus)
        sign = "+";
    else if (is_signed && spec->space)
        sign = " ";

    const char *prefix = "";
    if (spec->alternate && base == 16 && value != 0)
        prefix = capitals ? "0X" : "0x";
    /* Octal's `#` promises a leading zero, by one more if need be. */
    if (spec->alternate && base == 8 && zeros == 0 && (count == 0 || digits[at] != '0'))
        zeros = 1;

    size_t body = strlen(sign) + strlen(prefix) + zeros + count;
    size_t padding = (size_t)spec->width > body ? (size_t)spec->width - body : 0;
    /* `0` pads with zeros after the sign - unless `-` asked for the
     * left edge, or a precision said how many digits there are. */
    if (spec->zero && !spec->left && spec->precision < 0) {
        zeros += padding;
        padding = 0;
    }

    if (!spec->left)
        fill(out, ' ', padding);
    put(out, sign, strlen(sign));
    put(out, prefix, strlen(prefix));
    fill(out, '0', zeros);
    put(out, digits + at, count);
    if (spec->left)
        fill(out, ' ', padding);
}

/* ---- floating point ----
 *
 * printf("%.2f", 0.125) must say 0.12 and printf("%.2f", 0.375) 0.38:
 * both are exactly halfway, and C rounds a tie to the even digit. But
 * most doubles are not what they look like - 0.1 is really
 * 0.1000000000000000055511151231257827... - and to round one right
 * every digit of its exact value may matter. So it is worked out in
 * full first.
 *
 * Every double is a whole number m (53 bits at most) times a power of
 * two, 2^e. For e >= 0 that is a whole number: m doubled e times. For
 * e < 0 it is m / 2^-e - and since 1/2 = 5/10, that is m * 5^-e,
 * written with the decimal point -e digits from the right. Either way
 * the digits come from multiplying m by 2 or by 5, over and over, in an
 * integer as long as it needs to be: base 10^9, nine decimal digits to
 * each 32-bit "limb". The longest, m * 5^1074 for the smallest numbers,
 * is 767 digits. */

/* Up to 90 limbs of 9 digits: more than the longest needs. */
#define LIMBS 90
#define DIGITS (LIMBS * 9)

/* A number as decimal digits: 0.d1 d2 d3 ... times 10^point. Digits past
 * `count`, and before the first, are zeros; with count 0 it is 0. */
struct decimal {
    char digits[DIGITS + 1];
    int count;
    int point;
};

static char digit_at(const struct decimal *d, int i)
{
    return i >= 0 && i < d->count ? d->digits[i] : '0';
}

/* The exact digits of x, which is finite and not negative. */
static void exact_decimal(double x, struct decimal *d)
{
    uint64_t bits;
    memcpy(&bits, &x, sizeof bits);
    int biased = (int)(bits >> 52) & 0x7ff;
    uint64_t m = bits & ((UINT64_C(1) << 52) - 1);
    int e;
    if (biased == 0) {
        e = -1074; /* subnormal: no hidden bit */
    } else {
        m |= UINT64_C(1) << 52;
        e = biased - 1075;
    }
    if (m == 0) {
        d->count = 0;
        d->point = 1;
        return;
    }

    uint32_t limb[LIMBS]; /* the least significant first */
    int used = 0;
    while (m > 0) {
        limb[used++] = (uint32_t)(m % 1000000000);
        m /= 1000000000;
    }
    /* Multiply by 2 (or 5) as many times as it takes - several at once,
     * by the biggest power of it below 4 * 10^9, so a limb times it plus
     * the carry still fits in 64 bits. */
    int times = e >= 0 ? e : -e;
    uint32_t factor = e >= 0 ? 2 : 5;
    while (times > 0) {
        uint32_t by = 1;
        while (times > 0 && (uint64_t)by * factor < 4000000000u) {
            by *= factor;
            times--;
        }
        uint64_t carry = 0;
        for (int i = 0; i < used; i++) {
            uint64_t value = (uint64_t)limb[i] * by + carry;
            limb[i] = (uint32_t)(value % 1000000000);
            carry = value / 1000000000;
        }
        while (carry > 0) {
            limb[used++] = (uint32_t)(carry % 1000000000);
            carry /= 1000000000;
        }
    }

    /* The limbs as text: the top one without its leading zeros, every
     * other one as nine digits. */
    int count = 0;
    for (int i = used - 1; i >= 0; i--) {
        char nine[9];
        uint32_t value = limb[i];
        for (int k = 8; k >= 0; k--) {
            nine[k] = (char)('0' + value % 10);
            value /= 10;
        }
        int from = 0;
        if (i == used - 1)
            while (from < 8 && nine[from] == '0')
                from++;
        memcpy(d->digits + count, nine + from, (size_t)(9 - from));
        count += 9 - from;
    }
    d->count = count;
    d->point = count - (e < 0 ? -e : 0);
}

/* Keeps the first `keep` digits, rounded to the nearest - a tie, which
 * the exact digits make plain (a 5 and nothing after it), to the even
 * one. All of them carried up (9.96 to one decimal) makes one more
 * digit: 10.0. */
static void round_to(struct decimal *d, int keep)
{
    if (keep >= d->count)
        return;
    if (keep < 0) {
        d->count = 0; /* less than half the last digit kept: 0 */
        return;
    }
    char next = d->digits[keep];
    int up = next > '5';
    if (next == '5') {
        up = 0;
        for (int i = keep + 1; i < d->count; i++)
            if (d->digits[i] != '0')
                up = 1;
        if (!up)
            up = keep > 0 && (d->digits[keep - 1] - '0') % 2 == 1;
    }
    d->count = keep;
    if (!up)
        return;
    int i = keep - 1;
    while (i >= 0 && d->digits[i] == '9')
        d->digits[i--] = '0';
    if (i >= 0) {
        d->digits[i]++;
    } else {
        memmove(d->digits + 1, d->digits, (size_t)keep);
        d->digits[0] = '1';
        d->count = keep + 1;
        d->point++;
    }
}

/* The digits from `from`, `count` of them (zeros outside). */
static void put_digits(struct output *out, const struct decimal *d, int from, int count)
{
    char run[32];
    size_t n = 0;
    for (int i = 0; i < count; i++) {
        run[n++] = digit_at(d, from + i);
        if (n == sizeof run) {
            put(out, run, n);
            n = 0;
        }
    }
    put(out, run, n);
}

/* The last digit that is not 0, or -1 if there is none. */
static int last_nonzero(const struct decimal *d)
{
    int i = d->count - 1;
    while (i >= 0 && d->digits[i] == '0')
        i--;
    return i;
}

/* %f %e %g and their capitals. The pieces:
 *
 *     [spaces] [sign] [zeros] lead [.] fraction [e+XX] [spaces]
 *
 * where lead and fraction are runs of digits - for %f the whole part
 * and the decimals, for %e the first digit and the rest. */
static void put_float(struct output *out, const struct spec *spec, double value, char conversion)
{
    uint64_t bits;
    memcpy(&bits, &value, sizeof bits);
    int negative = (int)(bits >> 63);
    int capitals = conversion == 'F' || conversion == 'E' || conversion == 'G';
    const char *sign = negative ? "-" : spec->plus ? "+" : spec->space ? " " : "";
    size_t sign_length = strlen(sign);

    /* Infinity and NaN: words, padded with spaces only. */
    if (((bits >> 52) & 0x7ff) == 0x7ff) {
        int nan = (bits & ((UINT64_C(1) << 52) - 1)) != 0;
        const char *word = nan ? (capitals ? "NAN" : "nan") : (capitals ? "INF" : "inf");
        size_t body = sign_length + 3;
        size_t padding = (size_t)spec->width > body ? (size_t)spec->width - body : 0;
        if (!spec->left)
            fill(out, ' ', padding);
        put(out, sign, sign_length);
        put(out, word, 3);
        if (spec->left)
            fill(out, ' ', padding);
        return;
    }

    /* 800 bytes on the stack - not static, so two threads can print
     * numbers at once. */
    struct decimal d;
    exact_decimal(negative ? -value : value, &d);
    int precision = spec->precision < 0 ? 6 : spec->precision;
    char kind = (char)(conversion | 0x20); /* f, e or g */
    int lead_from, lead_count, fraction;
    int exponent = 0;
    int scientific = kind == 'e';

    if (kind == 'g') {
        /* %g: as many significant digits as the precision says (0 is
         * taken as 1); written as %e if the exponent that gives is
         * below -4 or not below the precision, else as %f - and then,
         * without #, the zeros at the end of the decimals dropped. */
        if (precision == 0)
            precision = 1;
        round_to(&d, precision);
        exponent = d.count == 0 ? 0 : d.point - 1;
        scientific = exponent < -4 || exponent >= precision;
        precision = scientific ? precision - 1 : precision - 1 - exponent;
    } else if (kind == 'e') {
        round_to(&d, precision + 1);
        exponent = d.count == 0 ? 0 : d.point - 1;
    } else {
        round_to(&d, d.point + precision);
    }

    if (scientific) {
        lead_from = d.count == 0 ? -1 : 0;
        lead_count = 1;
        fraction = precision;
        if (kind == 'g' && !spec->alternate) {
            int last = last_nonzero(&d);
            fraction = last < 1 ? 0 : (last < fraction ? last : fraction);
        }
    } else {
        /* The whole part: its digits, or one 0. */
        lead_from = d.point > 0 ? 0 : -1;
        lead_count = d.point > 0 ? d.point : 1;
        fraction = precision;
        if (kind == 'g' && !spec->alternate) {
            int needed = last_nonzero(&d) + 1 - d.point;
            if (needed < 0)
                needed = 0;
            if (needed < fraction)
                fraction = needed;
        }
    }
    int fraction_from = scientific ? (d.count == 0 ? -1 : 1) : d.point;
    int dot = fraction > 0 || spec->alternate;

    char tail[8] = "";
    size_t tail_length = 0;
    if (scientific) {
        /* e, the exponent's sign, and at least two digits of it. */
        int size = exponent < 0 ? -exponent : exponent;
        tail[tail_length++] = capitals ? 'E' : 'e';
        tail[tail_length++] = exponent < 0 ? '-' : '+';
        if (size >= 100)
            tail[tail_length++] = (char)('0' + size / 100);
        tail[tail_length++] = (char)('0' + size / 10 % 10);
        tail[tail_length++] = (char)('0' + size % 10);
    }

    size_t body = sign_length + (size_t)lead_count + (size_t)dot + (size_t)fraction + tail_length;
    size_t padding = (size_t)spec->width > body ? (size_t)spec->width - body : 0;
    size_t zeros = 0;
    if (spec->zero && !spec->left) {
        zeros = padding;
        padding = 0;
    }
    if (!spec->left)
        fill(out, ' ', padding);
    put(out, sign, sign_length);
    fill(out, '0', zeros);
    put_digits(out, &d, lead_from, lead_count);
    if (dot)
        put(out, ".", 1);
    put_digits(out, &d, fraction_from, fraction);
    put(out, tail, tail_length);
    if (spec->left)
        fill(out, ' ', padding);
}

int __taaj_format(emit_fn emit, void *context, const char *format, va_list arguments)
{
    struct output out = { emit, context, 0 };

    while (*format != '\0') {
        /* Ordinary text, up to the next %: as it is. */
        if (*format != '%') {
            const char *start = format;
            while (*format != '\0' && *format != '%')
                format++;
            put(&out, start, (size_t)(format - start));
            continue;
        }
        const char *conversion = format; /* kept, in case it is not one */
        format++;

        struct spec spec = { 0, 0, 0, 0, 0, 0, -1 };
        for (;; format++) {
            if (*format == '-')
                spec.left = 1;
            else if (*format == '0')
                spec.zero = 1;
            else if (*format == '+')
                spec.plus = 1;
            else if (*format == ' ')
                spec.space = 1;
            else if (*format == '#')
                spec.alternate = 1;
            else
                break;
        }

        /* `*` takes the number from the arguments. A negative width
         * means `-`; a negative precision means none was given. */
        if (*format == '*') {
            format++;
            spec.width = va_arg(arguments, int);
            if (spec.width < 0) {
                spec.left = 1;
                spec.width = -spec.width;
            }
        } else {
            spec.width = number(&format);
        }
        if (*format == '.') {
            format++;
            if (*format == '*') {
                format++;
                spec.precision = va_arg(arguments, int);
                if (spec.precision < 0)
                    spec.precision = -1;
            } else {
                spec.precision = number(&format);
            }
        }

        enum length length = PLAIN;
        switch (*format) {
        case 'h':
            format++;
            length = SHORT;
            if (*format == 'h') {
                format++;
                length = CHAR;
            }
            break;
        case 'l':
            format++;
            length = LONG;
            if (*format == 'l') {
                format++;
                length = LONG_LONG;
            }
            break;
        case 'z':
            format++;
            length = SIZE;
            break;
        case 'j':
            format++;
            length = MAX;
            break;
        case 't':
            format++;
            length = DIFFERENCE;
            break;
        case 'L':
            format++;
            length = LONG_DOUBLE;
            break;
        }

        switch (*format) {
        case 'd':
        case 'i': {
            /* Anything smaller than an int was made an int on its way
             * here (C does that to every argument after the dots), so
             * char and short are fetched as int and cut back down. */
            intmax_t value;
            switch (length) {
            case CHAR:       value = (signed char)va_arg(arguments, int); break;
            case SHORT:      value = (short)va_arg(arguments, int); break;
            case LONG:       value = va_arg(arguments, long); break;
            case LONG_LONG:  value = va_arg(arguments, long long); break;
            case SIZE:       value = (intmax_t)va_arg(arguments, size_t); break;
            case MAX:        value = va_arg(arguments, intmax_t); break;
            case DIFFERENCE: value = va_arg(arguments, ptrdiff_t); break;
            default:         value = va_arg(arguments, int); break;
            }
            uintmax_t size = value < 0 ? 0 - (uintmax_t)value : (uintmax_t)value;
            put_number(&out, &spec, size, value < 0, 1, 10, 0);
            break;
        }
        case 'u':
        case 'x':
        case 'X':
        case 'o': {
            uintmax_t value;
            switch (length) {
            case CHAR:       value = (unsigned char)va_arg(arguments, unsigned); break;
            case SHORT:      value = (unsigned short)va_arg(arguments, unsigned); break;
            case LONG:       value = va_arg(arguments, unsigned long); break;
            case LONG_LONG:  value = va_arg(arguments, unsigned long long); break;
            case SIZE:       value = va_arg(arguments, size_t); break;
            case MAX:        value = va_arg(arguments, uintmax_t); break;
            case DIFFERENCE: value = (uintmax_t)va_arg(arguments, ptrdiff_t); break;
            default:         value = va_arg(arguments, unsigned); break;
            }
            unsigned base = *format == 'u' ? 10 : *format == 'o' ? 8 : 16;
            put_number(&out, &spec, value, 0, 0, base, *format == 'X');
            break;
        }
        case 'p': {
            /* An address: hex with 0x, whatever the flags said. */
            uintptr_t address = (uintptr_t)va_arg(arguments, void *);
            spec.alternate = 0;
            spec.precision = -1;
            spec.zero = 0;
            char text[2 + 16];
            size_t at = sizeof text;
            do {
                text[--at] = "0123456789abcdef"[address % 16];
                address /= 16;
            } while (address > 0);
            text[--at] = 'x';
            text[--at] = '0';
            put_padded(&out, &spec, text + at, sizeof text - at);
            break;
        }
        case 'f':
        case 'F':
        case 'e':
        case 'E':
        case 'g':
        case 'G': {
            /* A float argument arrives as a double (the dots make it
             * one), so only long double is fetched apart. */
            double value = length == LONG_DOUBLE ? (double)va_arg(arguments, long double) : va_arg(arguments, double);
            put_float(&out, &spec, value, *format);
            break;
        }
        case 'c': {
            char byte = (char)va_arg(arguments, int);
            put_padded(&out, &spec, &byte, 1);
            break;
        }
        case 's': {
            const char *string = va_arg(arguments, const char *);
            if (string == NULL)
                string = "(null)";
            /* A precision is the most bytes to take - and then the
             * string need not even end in a zero. */
            size_t count = spec.precision < 0 ? strlen(string) : strnlen(string, (size_t)spec.precision);
            put_padded(&out, &spec, string, count);
            break;
        }
        case '%':
            put(&out, "%", 1);
            break;
        case '\0':
            /* The format ended inside a conversion: what there was of
             * it, and stop. */
            put(&out, conversion, (size_t)(format - conversion));
            return (int)out.total;
        default:
            /* Not one of ours: as it was written, so the mistake shows. */
            put(&out, conversion, (size_t)(format - conversion) + 1);
            break;
        }
        format++;
    }
    return (int)out.total;
}

/* snprintf and its relations: the text goes into the caller's buffer. */
struct buffer {
    char *to;
    size_t room; /* bytes that may be written, the zero not counted */
    size_t used;
};

static void into_buffer(void *context, const char *bytes, size_t count)
{
    struct buffer *buffer = context;
    size_t fits = buffer->room - buffer->used;
    if (count < fits)
        fits = count;
    /* With no room there may be no buffer at all: snprintf(NULL, 0, ...)
     * asks only for the length, and a null pointer is not to be added
     * to, even by nothing. */
    if (fits == 0)
        return;
    memcpy(buffer->to + buffer->used, bytes, fits);
    buffer->used += fits;
}

/* Writes at most size bytes, the zero included, and returns how long
 * the whole text is - which is how a caller learns it did not fit. */
int vsnprintf(char *to, size_t size, const char *format, va_list arguments)
{
    struct buffer buffer = { to, size > 0 ? size - 1 : 0, 0 };
    int total = __taaj_format(into_buffer, &buffer, format, arguments);
    if (size > 0)
        to[buffer.used] = '\0';
    return total;
}

int snprintf(char *to, size_t size, const char *format, ...)
{
    va_list arguments;
    va_start(arguments, format);
    int total = vsnprintf(to, size, format, arguments);
    va_end(arguments);
    return total;
}

/* The two that are not told the size of the buffer, and so cannot stay
 * inside it. Here because old programs call them; use snprintf. */
int vsprintf(char *to, const char *format, va_list arguments)
{
    return vsnprintf(to, SIZE_MAX, format, arguments);
}

int sprintf(char *to, const char *format, ...)
{
    va_list arguments;
    va_start(arguments, format);
    int total = vsnprintf(to, SIZE_MAX, format, arguments);
    va_end(arguments);
    return total;
}
