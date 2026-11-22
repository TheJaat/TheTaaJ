/* math.c - the exact part of math.h: rounding to whole numbers, the
 * remainder of a division, taking a number apart into its exponent and
 * fraction, the square root.
 *
 * "Exact" in the sense C means: each answer is the true result, rounded
 * once, as the hardware rounds +, -, * and /. Most of them never round
 * at all (floor, fmod, frexp: the true result is a double already); the
 * square root rounds once, to the nearest.
 *
 * Everything is done on the bits, with integer arithmetic, rather than
 * with the CPU's own instructions (SSE's roundsd and sqrtsd, AArch64's
 * frintm and fsqrt): it is the same C for both CPUs, and TCC, which
 * compiles this file on TheTaaJ, has neither the built-ins nor, on
 * AArch64, the instructions in its assembler.
 *
 * A double, as bits:
 *
 *     sign (1) | exponent (11) | fraction (52)
 *
 * a normal number is 1.fraction * 2^(exponent - 1023); exponent 0 is
 * zero or a subnormal (0.fraction * 2^-1022), exponent 2047 infinity
 * (fraction 0) or NaN. */

#include <limits.h>
#include <math.h>
#include <stdint.h>

#define SIGN     0x8000000000000000ULL
#define FRACTION 0x000fffffffffffffULL
#define HIDDEN   0x0010000000000000ULL /* the 1 in front of a normal number's fraction */

static uint64_t bits(double x)
{
    union { double d; uint64_t u; } v;
    v.d = x;
    return v.u;
}

static double from_bits(uint64_t u)
{
    union { double d; uint64_t u; } v;
    v.u = u;
    return v.d;
}

static int exponent_field(uint64_t u)
{
    return (int)((u >> 52) & 0x7ff);
}

/* For math.h's classification macros without compiler built-ins. */
int __taaj_fpclassify(double x)
{
    uint64_t u = bits(x);
    int e = exponent_field(u);
    if (e == 0x7ff)
        return (u & FRACTION) ? FP_NAN : FP_INFINITE;
    if (e == 0)
        return (u & FRACTION) ? FP_SUBNORMAL : FP_ZERO;
    return FP_NORMAL;
}

int __taaj_signbit(double x)
{
    return (int)(bits(x) >> 63);
}

double copysign(double x, double y)
{
    return from_bits((bits(x) & ~SIGN) | (bits(y) & SIGN));
}

float copysignf(float x, float y)
{
    return (float)copysign(x, y);
}

/* ---- whole numbers ---- */

/* Towards zero: clear the fraction's bits that are below the point. */
double trunc(double x)
{
    uint64_t u = bits(x);
    int e = exponent_field(u) - 1023;
    if (e >= 52)
        return x; /* no bits below the point - or infinite, or NaN */
    if (e < 0)
        return from_bits(u & SIGN); /* below 1: zero, of x's sign */
    uint64_t below = FRACTION >> e;
    return from_bits(u & ~below);
}

/* floor and ceil from trunc: they differ from it only for a number
 * with a fraction, by one, on one side of zero. t - 1 and t + 1 are
 * exact - t is a whole number under 2^52. A NaN fails every
 * comparison and comes back as it is. */
double floor(double x)
{
    double t = trunc(x);
    return t > x ? t - 1.0 : t;
}

double ceil(double x)
{
    double t = trunc(x);
    return t < x ? t + 1.0 : t;
}

/* Halves away from zero. x - t is exact (the fraction of x), so this
 * is right where x + 0.5 is not: 0.49999999999999994 + 0.5 rounds up
 * to 1. */
double round(double x)
{
    double t = trunc(x);
    double fraction = x - t;
    if (fraction >= 0.5)
        return t + 1.0;
    if (fraction <= -0.5)
        return t - 1.0;
    return t;
}

/* To the nearest, halves to even - the way the CPU rounds every sum.
 * Adding 2^52 leaves no bits below the point, so the CPU itself rounds
 * x to a whole number, and taking 2^52 away again is exact. (In the
 * current rounding mode, as C asks; TheTaaJ never changes it.) */
double rint(double x)
{
    const double two52 = 4503599627370496.0;
    uint64_t u = bits(x);
    if (exponent_field(u) - 1023 >= 52)
        return x;
    double y = x >= 0 ? (x + two52) - two52 : (x - two52) + two52;
    return copysign(y, x); /* -0.3 is -0, not 0 */
}

double nearbyint(double x)
{
    return rint(x);
}

long lround(double x) { return (long)round(x); }
long long llround(double x) { return (long long)round(x); }
long lrint(double x) { return (long)rint(x); }
long long llrint(double x) { return (long long)rint(x); }

float truncf(float x) { return (float)trunc(x); }
float floorf(float x) { return (float)floor(x); }
float ceilf(float x) { return (float)ceil(x); }
float roundf(float x) { return (float)round(x); }
float rintf(float x) { return (float)rint(x); }
float nearbyintf(float x) { return (float)rint(x); }
long lroundf(float x) { return (long)round(x); }
long long llroundf(float x) { return (long long)round(x); }
long lrintf(float x) { return (long)rint(x); }
long long llrintf(float x) { return (long long)rint(x); }

/* ---- remainders ---- */

/* x - n*y for the n that is x/y rounded towards zero: always exact.
 * Long division in binary on the two fractions, as integers: subtract
 * y's from x's wherever it fits, then shift, once per power of two the
 * exponents differ by. */
double fmod(double x, double y)
{
    uint64_t ux = bits(x), uy = bits(y);
    int ex = exponent_field(ux), ey = exponent_field(uy);
    uint64_t sign = ux & SIGN;

    /* fmod(x, 0), fmod(infinity, y) and NaNs: NaN. */
    if ((uy << 1) == 0 || ex == 0x7ff || (ey == 0x7ff && (uy & FRACTION)))
        return (x * y) / (x * y);
    /* |x| < |y| (or y infinite): x itself. */
    if ((ux << 1) <= (uy << 1))
        return (ux << 1) == (uy << 1) ? 0 * x : x;

    /* The fractions as integers with the hidden 1 in place; a
     * subnormal's is shifted up until it has one, the exponent going
     * below 1 to match. */
    if (ex == 0) {
        for (uint64_t i = ux << 12; (i >> 63) == 0; i <<= 1)
            ex--;
        ux <<= 1 - ex;
    } else {
        ux = (ux & FRACTION) | HIDDEN;
    }
    if (ey == 0) {
        for (uint64_t i = uy << 12; (i >> 63) == 0; i <<= 1)
            ey--;
        uy <<= 1 - ey;
    } else {
        uy = (uy & FRACTION) | HIDDEN;
    }

    for (; ex > ey; ex--) {
        if (ux >= uy) {
            ux -= uy;
            if (ux == 0)
                return 0 * x;
        }
        ux <<= 1;
    }
    if (ux >= uy) {
        ux -= uy;
        if (ux == 0)
            return 0 * x;
    }

    /* Put the result back together: shift the fraction up to the
     * hidden bit, then make it normal or, if the exponent is too low,
     * subnormal. */
    for (; (ux >> 52) == 0; ux <<= 1)
        ex--;
    if (ex > 0)
        ux = (ux - HIDDEN) | ((uint64_t)ex << 52);
    else
        ux >>= 1 - ex;
    return from_bits(ux | sign);
}

/* x - n*y for the n that is x/y rounded to the nearest, halves to
 * even: the result is between -|y|/2 and |y|/2. From fmod by 2|y|,
 * which is in [0, 2|y|) and keeps n's evenness; then at most two
 * subtractions of |y|. */
double remainder(double x, double y)
{
    double p = fabs(y);
    int negative = signbit(x);
    if (isnan(x) || isnan(y) || isinf(x) || p == 0)
        return (x * y) / (x * y);
    if (p <= 8.98846567431157953865e307) /* DBL_MAX / 2: 2p is finite */
        x = fmod(x, p + p);
    x = fabs(x);
    if (p < 4.4501477170144028e-308) { /* 2 * DBL_MIN: p/2 is not exact */
        if (x + x > p) {
            x -= p;
            if (x + x >= p)
                x -= p;
        }
    } else {
        double half = 0.5 * p;
        if (x > half) {
            x -= p;
            if (x >= half)
                x -= p;
        }
    }
    return negative ? -x : x;
}

float fmodf(float x, float y) { return (float)fmod(x, y); }
float remainderf(float x, float y) { return (float)remainder(x, y); }

/* ---- the larger, the smaller, the difference ---- */

/* A NaN is "missing data": the other one is the answer. Of 0 and -0,
 * which compare equal, either may be given (C leaves it open): here,
 * as in glibc, the first. */
double fmin(double x, double y)
{
    if (isnan(x))
        return y;
    if (isnan(y))
        return x;
    return x <= y ? x : y;
}

double fmax(double x, double y)
{
    if (isnan(x))
        return y;
    if (isnan(y))
        return x;
    return x >= y ? x : y;
}

double fdim(double x, double y)
{
    if (isnan(x) || isnan(y))
        return x + y;
    return x > y ? x - y : 0.0;
}

float fminf(float x, float y) { return (float)fmin(x, y); }
float fmaxf(float x, float y) { return (float)fmax(x, y); }
float fdimf(float x, float y) { return (float)fdim(x, y); }

/* ---- exponent and fraction ---- */

/* x = fraction * 2^exponent, fraction in [0.5, 1). */
double frexp(double x, int *exponent)
{
    uint64_t u = bits(x);
    int e = exponent_field(u);
    if (e == 0) {
        if ((u << 1) == 0) {
            *exponent = 0;
            return x;
        }
        /* Subnormal: scale it up into the normal numbers first. */
        x = frexp(x * 18446744073709551616.0, exponent); /* 2^64 */
        *exponent -= 64;
        return x;
    }
    if (e == 0x7ff) {
        *exponent = 0;
        return x;
    }
    *exponent = e - 1022;
    return from_bits((u & ~(0x7ffULL << 52)) | (1022ULL << 52));
}

float frexpf(float x, int *exponent)
{
    return (float)frexp(x, exponent);
}

double scalbn(double x, int n) { return ldexp(x, n); }
float scalbnf(float x, int n) { return ldexpf(x, n); }

double scalbln(double x, long n)
{
    if (n > INT_MAX)
        n = INT_MAX;
    if (n < INT_MIN)
        n = INT_MIN;
    return ldexp(x, (int)n);
}

float scalblnf(float x, long n) { return (float)scalbln(x, n); }

/* The exponent, as in x = 1.f * 2^exponent. */
int ilogb(double x)
{
    if (isnan(x))
        return FP_ILOGBNAN;
    if (isinf(x))
        return INT_MAX;
    if (x == 0)
        return FP_ILOGB0;
    int e;
    frexp(x, &e);
    return e - 1;
}

double logb(double x)
{
    if (isnan(x))
        return x;
    if (isinf(x))
        return fabs(x);
    if (x == 0)
        return -HUGE_VAL; /* and C says: a division by zero */
    return (double)ilogb(x);
}

int ilogbf(float x) { return ilogb(x); }
float logbf(float x) { return (float)logb(x); }

/* The whole part and the fraction, each with x's sign. */
double modf(double x, double *whole)
{
    *whole = trunc(x);
    if (isinf(x))
        return copysign(0.0, x);
    return copysign(x - *whole, x);
}

float modff(float x, float *whole)
{
    double w;
    float fraction = (float)modf(x, &w);
    *whole = (float)w;
    return fraction;
}

/* The tag says which NaN; there is only one kind here. */
double nan(const char *tag) { (void)tag; return NAN; }
float nanf(const char *tag) { (void)tag; return NAN; }
long double nanl(const char *tag) { (void)tag; return NAN; }

/* The next double after x, towards y: one more or one less in the
 * bits, which for doubles of one sign is the next number along. */
double nextafter(double x, double y)
{
    if (isnan(x) || isnan(y))
        return x + y;
    if (x == y)
        return y;
    if (x == 0)
        return copysign(from_bits(1), y); /* the smallest subnormal */
    uint64_t u = bits(x);
    if ((x < y) == (x > 0))
        u++;
    else
        u--;
    return from_bits(u);
}

float nextafterf(float x, float y)
{
    union { float f; uint32_t u; } v;
    if (isnan(x) || isnan(y))
        return x + y;
    if (x == y)
        return y;
    if (x == 0) {
        v.u = 1;
        return y > 0 ? v.f : -v.f;
    }
    v.f = x;
    if ((x < y) == (x > 0))
        v.u++;
    else
        v.u--;
    return v.f;
}

/* ---- the square root ---- */

/* Correctly rounded: the root of the fraction is worked out one bit at
 * a time, exactly as square roots are done by hand in decimal - bring
 * down two digits, find the largest next digit whose trial square
 * fits, subtract - but in base two, where the next digit is 1 if
 * 4r + 1 fits and 0 if not. 54 bits of root: the 53 a double holds,
 * and one more to round by. (A square root is never exactly half-way
 * between two doubles, so that bit decides alone.) */
double sqrt(double x)
{
    uint64_t u = bits(x);
    int e = exponent_field(u);
    if (e == 0x7ff)
        return (u & SIGN) && !(u & FRACTION) ? (x - x) / (x - x) : x; /* NaN; sqrt(+inf) = +inf; sqrt(-inf) = NaN */
    if ((u << 1) == 0)
        return x; /* sqrt(-0) is -0 */
    if (u & SIGN)
        return (x - x) / (x - x); /* a negative number: NaN */

    /* m * 2^e, with m an integer of 53 bits (a subnormal's shifted up
     * to have them). */
    uint64_t m;
    if (e == 0) {
        m = u & FRACTION;
        e = 1;
        while (!(m & HIDDEN)) {
            m <<= 1;
            e--;
        }
    } else {
        m = (u & FRACTION) | HIDDEN;
    }
    e -= 1023 + 52; /* now x = m * 2^e exactly */
    /* An even exponent halves cleanly. */
    if (e & 1) {
        m <<= 1;
        e--;
    }
    /* sqrt(m * 2^e) = sqrt(m * 2^56) * 2^((e - 56) / 2): the root of
     * m * 2^56 (about 2^108 or 2^109) has 54 or 55 bits. The bits are
     * fed in two at a time from the top of m, then zeros. */
    uint64_t root = 0, rest = 0;
    int pairs = 55; /* (54 bits of m, rounded up to pairs) / 2 + 56 / 2 */
    for (int i = 0; i < pairs; i++) {
        int shift = 52 - 2 * i; /* where the next pair of m's bits is */
        uint64_t two = shift >= 0 ? (m >> shift) & 3 : 0;
        rest = (rest << 2) | two;
        uint64_t trial = (root << 2) | 1;
        if (rest >= trial) {
            rest -= trial;
            root = (root << 1) | 1;
        } else {
            root <<= 1;
        }
    }
    /* root is sqrt(m * 2^56) rounded down: m is between 2^52 and 2^54,
     * so root is between 2^54 and 2^55 - 55 bits, of which the value
     * is root * 2^((e - 56) / 2). One bit more than the 54 wanted:
     * drop it, and anything it held makes no difference (only a tie
     * could, and there are none). */
    int exponent = (e - 56) / 2 + 1;
    root >>= 1;
    /* Now value = root * 2^exponent, root of 54 bits. Round off the
     * last one: up if it is 1. */
    uint64_t result = (root >> 1) + (root & 1);
    exponent++;
    if (result >> 53) { /* rounding carried into a new bit */
        result >>= 1;
        exponent++;
    }
    return ldexp((double)result, exponent);
}

float sqrtf(float x)
{
    /* Rounding twice is safe here: a double has more than twice a
     * float's bits, plus two. */
    return (float)sqrt(x);
}

/* sqrt(x*x + y*y) without x*x overflowing when x is big, or vanishing
 * when it is small: both are scaled by a power of two first (which is
 * exact) and back after. Within about one rounding of the true value,
 * not always the nearest double. An infinity wins even over a NaN. */
double hypot(double x, double y)
{
    if (isinf(x) || isinf(y))
        return HUGE_VAL;
    if (isnan(x) || isnan(y))
        return x + y;
    double a = fabs(x), b = fabs(y);
    if (a < b) {
        double t = a;
        a = b;
        b = t;
    }
    if (b == 0)
        return a;
    if (a > b * 1152921504606846976.0) /* 2^60: b is lost in a's rounding */
        return a + b;
    int scale = 0;
    /* a * a must stay between the smallest normal double and the
     * largest: a below about 2^500, b above about 2^-500. */
    if (a > 1e150) {
        a = ldexp(a, -600);
        b = ldexp(b, -600);
        scale = 600;
    } else if (b < 1e-150) {
        a = ldexp(a, 600);
        b = ldexp(b, 600);
        scale = -600;
    }
    return ldexp(sqrt(a * a + b * b), scale);
}

float hypotf(float x, float y)
{
    double a = x, b = y;
    if (isinf(a) || isinf(b))
        return HUGE_VALF;
    return (float)sqrt(a * a + b * b);
}
