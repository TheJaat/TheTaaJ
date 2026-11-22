/* stdlib.c - the parts of stdlib.h that need nothing from the system:
 * text into numbers, sorting, and searching what is sorted.
 *
 * (malloc and exit, which do need the system, are in the Rust half:
 * libc/src/memory.rs and lib.rs.) */

#include <stdlib.h>

#include <ctype.h>
#include <errno.h>
#include <limits.h>

/* ---- text into numbers ---- */

/* What a digit is worth: 0 to 9, then a (or A) is 10, up to z, 35.
 * Anything else is worth more than any base. */
static int worth(int c)
{
    if (isdigit(c))
        return c - '0';
    if (islower(c))
        return c - 'a' + 10;
    if (isupper(c))
        return c - 'A' + 10;
    return 99;
}

/* The work of strtol and strtoul. Reads
 *
 *     spaces   an optional + or -   an optional 0x or 0   digits
 *
 * and gives the digits' value in *value - without the sign, which is
 * reported in *negative, because where "too big" lies depends on the
 * sign and on which of the two functions is asking. *end is left just
 * after the last digit, or at the very start of the text if there were
 * no digits at all. Returns 0, or -1 if the value did not fit even an
 * unsigned long. */
static int parse(const char *text, char **end, int base, unsigned long *value, int *negative)
{
    const char *at = text;
    *value = 0;
    *negative = 0;
    while (isspace((unsigned char)*at))
        at++;
    if (*at == '+' || *at == '-')
        *negative = *at++ == '-';
    /* 0x means sixteen only if a hex digit follows: "0x" alone is the
     * number 0 and then an x. */
    if ((base == 0 || base == 16) && at[0] == '0' && (at[1] == 'x' || at[1] == 'X') && isxdigit((unsigned char)at[2])) {
        at += 2;
        base = 16;
    } else if (base == 0) {
        base = at[0] == '0' ? 8 : 10;
    }
    int any = 0;
    int too_big = 0;
    for (;; at++) {
        int digit = worth((unsigned char)*at);
        if (digit >= base)
            break;
        any = 1;
        /* value * base + digit, unless that is more than fits. */
        if (*value > (ULONG_MAX - (unsigned long)digit) / (unsigned long)base)
            too_big = 1;
        else
            *value = *value * (unsigned long)base + (unsigned long)digit;
    }
    if (end != NULL)
        *end = (char *)(any ? at : text);
    return too_big ? -1 : 0;
}

static int bad_base(int base)
{
    return base < 0 || base == 1 || base > 36;
}

long strtol(const char *text, char **end, int base)
{
    unsigned long value;
    int negative;
    if (bad_base(base)) {
        errno = EINVAL;
        return 0;
    }
    int too_big = parse(text, end, base, &value, &negative);
    /* A long reaches one further below zero than above it. */
    unsigned long most = negative ? (unsigned long)LONG_MAX + 1 : (unsigned long)LONG_MAX;
    if (too_big || value > most) {
        errno = ERANGE;
        return negative ? LONG_MIN : LONG_MAX;
    }
    /* Negated as an unsigned number, where it cannot overflow, and
     * then read as a signed one. */
    return (long)(negative ? 0 - value : value);
}

unsigned long strtoul(const char *text, char **end, int base)
{
    unsigned long value;
    int negative;
    if (bad_base(base)) {
        errno = EINVAL;
        return 0;
    }
    if (parse(text, end, base, &value, &negative) < 0) {
        errno = ERANGE;
        return ULONG_MAX;
    }
    /* "-1" is allowed, and is the biggest number there is: C says the
     * minus sign negates the result in the unsigned type. */
    return negative ? 0 - value : value;
}

/* long long is the same size as long on both of TheTaaJ's machines,
 * so these are strtol and strtoul under the other names. */
long long strtoll(const char *text, char **end, int base)
{
    return strtol(text, end, base);
}

unsigned long long strtoull(const char *text, char **end, int base)
{
    return strtoul(text, end, base);
}

/* The two that cannot say they failed. Use strtol. */
long atol(const char *text)
{
    return strtol(text, NULL, 10);
}

int atoi(const char *text)
{
    return (int)strtol(text, NULL, 10);
}

int abs(int number)
{
    return number < 0 ? -number : number;
}

long labs(long number)
{
    return number < 0 ? -number : number;
}

long long llabs(long long number)
{
    return number < 0 ? -number : number;
}

long long atoll(const char *text)
{
    return strtoll(text, NULL, 10);
}

/* C99 says the quotient is rounded towards zero, as `/` does, so the
 * remainder has the numerator's sign: div(-7, 2) is {-3, -1}. */
div_t div(int numerator, int denominator)
{
    div_t result = {numerator / denominator, numerator % denominator};
    return result;
}

ldiv_t ldiv(long numerator, long denominator)
{
    ldiv_t result = {numerator / denominator, numerator % denominator};
    return result;
}

lldiv_t lldiv(long long numerator, long long denominator)
{
    lldiv_t result = {numerator / denominator, numerator % denominator};
    return result;
}

/* ---- sorting ---- */

/* qsort knows nothing about what it sorts: only how many items, how
 * big each is, and a function that compares two. So an item is "size
 * bytes at this address", and moving one is moving bytes. */
static void exchange(unsigned char *one, unsigned char *other, size_t size)
{
    while (size-- > 0) {
        unsigned char byte = *one;
        *one++ = *other;
        *other++ = byte;
    }
}

/* The items are treated as a tree laid out in the array: the children
 * of item n are items 2n+1 and 2n+2. A "heap" is such a tree in which
 * no child is greater than its parent - so the greatest is at the top.
 *
 * This lets the item at `top` sink to where it belongs: while a child
 * is greater than it, it changes places with its greater child. */
static void sink(unsigned char *items, size_t top, size_t count, size_t size,
                 int (*compare)(const void *, const void *))
{
    for (;;) {
        size_t child = 2 * top + 1;
        if (child >= count)
            return;
        if (child + 1 < count && compare(items + child * size, items + (child + 1) * size) < 0)
            child++;
        if (compare(items + top * size, items + child * size) >= 0)
            return;
        exchange(items + top * size, items + child * size, size);
        top = child;
    }
}

/* Heap sort. Despite its name, qsort need not be quicksort, and this
 * is not: quicksort is faster on a good day, but on a bad one - items
 * that arrive in an unlucky order - it takes time that grows with the
 * square of their number, and calls itself that deep, on a stack that
 * here is small. Heap sort takes the same time whatever it is given,
 * and calls nothing.
 *
 * First the array is made into a heap. Then, again and again: the top
 * (the greatest left) changes places with the last item, the heap is
 * one shorter, and the new top sinks. */
void qsort(void *base, size_t count, size_t size, int (*compare)(const void *, const void *))
{
    unsigned char *items = base;
    if (count < 2 || size == 0)
        return;
    for (size_t top = count / 2; top-- > 0;)
        sink(items, top, count, size, compare);
    for (size_t last = count - 1; last > 0; last--) {
        exchange(items, items + last * size, size);
        sink(items, 0, last, size, compare);
    }
}

/* Looks for `key` among sorted items by halving: compare with the
 * middle one, and go on in the half it must be in. A thousand items
 * take ten comparisons. Returns the item, or NULL. */
void *bsearch(const void *key, const void *base, size_t count, size_t size,
              int (*compare)(const void *, const void *))
{
    const unsigned char *items = base;
    size_t low = 0;
    size_t high = count; /* the item, if it is there, is in [low, high) */
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        const unsigned char *item = items + middle * size;
        int order = compare(key, item);
        if (order == 0)
            return (void *)item;
        if (order < 0)
            high = middle;
        else
            low = middle + 1;
    }
    return NULL;
}

/* rand and srand: the generator the C standard gives as its example,
 * so that a program gets the same numbers here as the standard says. */
static unsigned long rand_next = 1;

int rand(void)
{
    rand_next = rand_next * 1103515245 + 12345;
    return (int)((unsigned)(rand_next / 65536) % 32768);
}

void srand(unsigned seed)
{
    rand_next = seed;
}
