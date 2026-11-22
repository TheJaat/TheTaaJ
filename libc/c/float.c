/* float.c - text into floating-point numbers, and scaling by powers
 * of two, and the absolute value: strtod, strtof, strtold, atof;
 * ldexp, ldexpf, ldexpl; fabs, fabsf, fabsl.
 *
 * Finding the double nearest to some decimal text is done in the Rust
 * half (libc/src/system.rs), by Rust's own correctly rounded parser,
 * which hands back the number's bits as an integer. That is because
 * the Rust half is built without floating-point registers and would
 * return a double where C does not look for one; an integer comes back
 * the same way from both. Here the bits become a double again. */

#include <errno.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

uint64_t __taaj_parse_double(const char *text, char **end);
uint32_t __taaj_parse_float(const char *text, char **end);

double strtod(const char *text, char **end)
{
    uint64_t bits = __taaj_parse_double(text, end);
    double value;
    memcpy(&value, &bits, sizeof value);
    return value;
}

float strtof(const char *text, char **end)
{
    uint32_t bits = __taaj_parse_float(text, end);
    float value;
    memcpy(&value, &bits, sizeof value);
    return value;
}

/* A long double from the nearest double: exact for anything a double
 * holds, but not the nearest long double to text with more digits than
 * a double keeps (17). */
long double strtold(const char *text, char **end)
{
    return strtod(text, end);
}

double atof(const char *text)
{
    return strtod(text, NULL);
}

/* x times 2 to the power n. Multiplying by a power of two only changes
 * the exponent, so it is exact - until the result is too big (it
 * becomes infinite) or so small that bits fall off the bottom. The
 * power is applied in steps that each fit in the type, so that 2^n
 * itself never overflows on the way: ldexp(0x1p-1074, 2000) is 2^926,
 * though 2^2000 is not a double. */
long double ldexpl(long double x, int n)
{
    while (n > 1000) {
        x *= 0x1p1000L;
        n -= 1000;
        if (x == 0 || x != x || x - x != 0)
            return x; /* 0, NaN or infinite: scaling changes nothing more */
    }
    while (n < -1000) {
        x *= 0x1p-1000L;
        n += 1000;
        if (x == 0 || x != x || x - x != 0)
            return x;
    }
    long double power = 1;
    long double step = n < 0 ? 0.5L : 2.0L;
    for (int k = n < 0 ? -n : n; k > 0; k >>= 1) {
        if (k & 1)
            power *= step;
        step *= step;
    }
    return x * power;
}

double ldexp(double x, int n)
{
    if (n > 2200)
        n = 2200; /* enough to make anything but 0 infinite */
    if (n < -2200)
        n = -2200;
    return (double)ldexpl(x, n);
}

float ldexpf(float x, int n)
{
    return (float)ldexp(x, n);
}

/* The absolute value: the number with its sign bit cleared - which is
 * also right for -0.0 (it gives 0.0) and keeps a NaN a NaN, where
 * "x < 0 ? -x : x" gets both wrong. Clang never calls these (math.h
 * turns them into its built-ins); TCC does. The parentheses round the
 * names keep math.h's macros from expanding here. */
double (fabs)(double x)
{
    uint64_t b;
    memcpy(&b, &x, sizeof b);
    b &= ~(UINT64_C(1) << 63);
    memcpy(&x, &b, sizeof b);
    return x;
}

float (fabsf)(float x)
{
    uint32_t b;
    memcpy(&b, &x, sizeof b);
    b &= ~(UINT32_C(1) << 31);
    memcpy(&x, &b, sizeof b);
    return x;
}

/* A long double is the x87's 80-bit number on the PC, its sign in the
 * top bit of byte 9 (the rest of the 16 bytes are padding), and a
 * 128-bit number on AArch64, its sign in the top bit of byte 15. */
#if defined(__x86_64__)
#define LONG_DOUBLE_SIGN_BYTE 9
#else
#define LONG_DOUBLE_SIGN_BYTE 15
#endif

long double (fabsl)(long double x)
{
    unsigned char b[sizeof x];
    memcpy(b, &x, sizeof x);
    b[LONG_DOUBLE_SIGN_BYTE] &= 0x7f;
    memcpy(&x, b, sizeof x);
    return x;
}
