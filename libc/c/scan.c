/* scan.c - sscanf: reading numbers and words out of a string, by a
 * format like printf's, the other way round.
 *
 *   sscanf("12 apples, 3.5 kg", "%d %s %lf", &n, word, &kg)
 *
 * sets n to 12, word to "apples," and kg to 3.5, and returns 3: how
 * many it set. It stops at the first thing that does not match, and
 * returns how many it had set by then - or EOF if the text ran out
 * before the first.
 *
 * What the format can say:
 *
 *   a space        any amount of white space in the text, even none
 *   a character    that character
 *   %%             a %
 *   %d %i %u %o %x integers: ten, as C reads them (0x..., 0...), ten
 *                  unsigned, eight, sixteen
 *   %f %e %g %a    a floating-point number, as strtod reads it
 *   %s             a word: up to the next white space
 *   %c             one character - or as many as the width says -
 *                  white space included, and no zero after them
 *   %[abc] %[^abc] the longest run of the listed characters (or of
 *                  any others), as a string; %[]..] lists the ]
 *   %p             a pointer, as %p printed it
 *   %n             nothing read: stores how many characters have been
 *
 * and between the % and the letter: * (read it, store it nowhere), a
 * width (read at most that many characters), and the size of what the
 * pointer points to - hh h l ll j z t for integers, l and L for double
 * and long double.
 *
 * Only from a string. (A scanf from a stream needs to push back what
 * it read too far, and that is not here yet.) */

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The sizes %d and its kin can store. */
enum size { SIZE_CHAR, SIZE_SHORT, SIZE_INT, SIZE_LONG, SIZE_LONG_LONG, SIZE_LONG_DOUBLE };

/* Stores an integer through the next argument, in the size it names. */
static void store_integer(va_list *arguments, enum size size, unsigned long long value)
{
    switch (size) {
    case SIZE_CHAR: *va_arg(*arguments, char *) = (char)value; break;
    case SIZE_SHORT: *va_arg(*arguments, short *) = (short)value; break;
    case SIZE_INT: *va_arg(*arguments, int *) = (int)value; break;
    case SIZE_LONG: *va_arg(*arguments, long *) = (long)value; break;
    default: *va_arg(*arguments, long long *) = (long long)value; break;
    }
}

/* Is c in the set of a %[...]? The set is from `set` to `end`, its
 * leading ^ already taken off. */
static int in_set(const char *set, const char *end, unsigned char c)
{
    for (const char *s = set; s < end; s++) {
        /* a-z: a range, unless the - is first or last */
        if (s + 2 < end && s[1] == '-') {
            if ((unsigned char)s[0] <= c && c <= (unsigned char)s[2])
                return 1;
            s += 2;
        } else if ((unsigned char)*s == c) {
            return 1;
        }
    }
    return 0;
}

int vsscanf(const char *text, const char *format, va_list given)
{
    va_list arguments;
    va_copy(arguments, given);
    const char *in = text;
    int stored = 0;
    int converted_any = 0; /* EOF is only for running out before the first */

    for (const char *f = format; *f; f++) {
        if (isspace((unsigned char)*f)) {
            while (isspace((unsigned char)*in))
                in++;
            continue;
        }
        if (*f != '%' || f[1] == '%') {
            if (*f == '%')
                f++;
            if (*in == '\0')
                goto out_of_text;
            if (*in != *f)
                break;
            in++;
            continue;
        }

        /* A conversion: %, then *, width, size, letter. */
        f++;
        int suppress = 0;
        if (*f == '*') {
            suppress = 1;
            f++;
        }
        size_t width = 0;
        while (isdigit((unsigned char)*f))
            width = width * 10 + (size_t)(*f++ - '0');
        enum size size = SIZE_INT;
        switch (*f) {
        case 'h':
            f++;
            size = SIZE_SHORT;
            if (*f == 'h') {
                f++;
                size = SIZE_CHAR;
            }
            break;
        case 'l':
            f++;
            size = SIZE_LONG;
            if (*f == 'l') {
                f++;
                size = SIZE_LONG_LONG;
            }
            break;
        case 'L': f++; size = SIZE_LONG_DOUBLE; break;
        case 'j': case 'z': case 't': f++; size = SIZE_LONG; break; /* all 64-bit here */
        default: break;
        }
        char letter = *f;
        if (letter == '\0')
            break;

        if (letter == 'n') {
            if (!suppress)
                store_integer(&arguments, size, (unsigned long long)(in - text));
            continue;
        }
        /* Everything but %c and %[ skips white space first. */
        if (letter != 'c' && letter != '[')
            while (isspace((unsigned char)*in))
                in++;
        if (*in == '\0')
            goto out_of_text;

        switch (letter) {
        case 'd': case 'i': case 'u': case 'o': case 'x': case 'X': case 'p': {
            /* The number's characters, at most `width` of them, copied
             * out so that strtoull cannot read past the width. */
            char digits[72];
            size_t limit = width && width < sizeof digits ? width : sizeof digits - 1;
            size_t n = 0;
            while (n < limit && in[n] && !isspace((unsigned char)in[n]))
                n++;
            memcpy(digits, in, n);
            digits[n] = '\0';
            int base = letter == 'd' || letter == 'u' ? 10 : letter == 'o' ? 8 : letter == 'i' ? 0 : 16;
            /* "0x" with no hex digit after it (within the width) is not
             * a number: the longest start of one C can read there is
             * "0x", which is not one itself - a matching failure, as
             * newer glibc says too (2.43 does; 2.39 still took the 0).
             * strtoull would take the 0. */
            if (base == 16 || base == 0) {
                const char *p = digits[0] == '+' || digits[0] == '-' ? digits + 1 : digits;
                if (p[0] == '0' && (p[1] == 'x' || p[1] == 'X') && !isxdigit((unsigned char)p[2]))
                    goto done;
            }
            char *end;
            unsigned long long value;
            if (letter == 'd' || letter == 'i')
                value = (unsigned long long)strtoll(digits, &end, base);
            else
                value = strtoull(digits, &end, base);
            if (end == digits)
                goto done; /* no number here */
            in += end - digits;
            if (!suppress) {
                if (letter == 'p')
                    *va_arg(arguments, void **) = (void *)(uintptr_t)value;
                else
                    store_integer(&arguments, size, value);
                stored++;
            }
            break;
        }
        case 'f': case 'F': case 'e': case 'E': case 'g': case 'G': case 'a': case 'A': {
            char number[512];
            size_t limit = width && width < sizeof number ? width : sizeof number - 1;
            size_t n = 0;
            while (n < limit && in[n] && !isspace((unsigned char)in[n]))
                n++;
            memcpy(number, in, n);
            number[n] = '\0';
            char *end;
            if (size == SIZE_LONG_DOUBLE) {
                long double value = strtold(number, &end);
                if (end != number && !suppress)
                    *va_arg(arguments, long double *) = value;
            } else if (size == SIZE_LONG) {
                double value = strtod(number, &end);
                if (end != number && !suppress)
                    *va_arg(arguments, double *) = value;
            } else {
                float value = strtof(number, &end);
                if (end != number && !suppress)
                    *va_arg(arguments, float *) = value;
            }
            if (end == number)
                goto done;
            in += end - number;
            if (!suppress)
                stored++;
            break;
        }
        case 's': {
            char *to = suppress ? NULL : va_arg(arguments, char *);
            size_t n = 0;
            while (in[n] && !isspace((unsigned char)in[n]) && (!width || n < width)) {
                if (to)
                    to[n] = in[n];
                n++;
            }
            if (to) {
                to[n] = '\0';
                stored++;
            }
            in += n;
            break;
        }
        case 'c': {
            size_t count = width ? width : 1;
            char *to = suppress ? NULL : va_arg(arguments, char *);
            size_t n = 0;
            while (n < count && in[n]) {
                if (to)
                    to[n] = in[n];
                n++;
            }
            if (n < count)
                goto out_of_text; /* the text ended inside them: C calls that running out, not a mismatch */
            in += n;
            if (to)
                stored++;
            break;
        }
        case '[': {
            f++;
            int negated = 0;
            if (*f == '^') {
                negated = 1;
                f++;
            }
            const char *set = f;
            if (*f == ']') /* a ] first is one of the set */
                f++;
            while (*f && *f != ']')
                f++;
            if (*f == '\0')
                goto done; /* the format never closed the set */
            const char *end = f;
            char *to = suppress ? NULL : va_arg(arguments, char *);
            size_t n = 0;
            while (in[n] && (!width || n < width) && in_set(set, end, (unsigned char)in[n]) != negated) {
                if (to)
                    to[n] = in[n];
                n++;
            }
            if (n == 0)
                goto done;
            if (to) {
                to[n] = '\0';
                stored++;
            }
            in += n;
            break;
        }
        default:
            goto done; /* not a conversion C knows */
        }
        converted_any = 1;
    }
done:
    va_end(arguments);
    return stored;

out_of_text:
    va_end(arguments);
    return converted_any || stored ? stored : EOF;
}

int sscanf(const char *text, const char *format, ...)
{
    va_list arguments;
    va_start(arguments, format);
    int stored = vsscanf(text, format, arguments);
    va_end(arguments);
    return stored;
}
