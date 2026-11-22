/* math_test.c - the library's exact mathematics, on this machine.
 *
 * Runs on the host, not in TheTaaJ. math.c is compiled for the host as
 * it is for TheTaaJ, and then every name in it gets "taaj_" in front
 * (objcopy --prefix-symbols): what it defines - taaj_floor, taaj_sqrt -
 * and what it calls, taaj_ldexp, which this file passes on to the
 * host's. So the two libraries live in one program, and each function
 * is given the same numbers as the host's (glibc's) and must give the
 * same answer, bit for bit: a NaN must be a NaN, and 0 and -0 differ.
 *
 * glibc's are all exact too, so "the same bits" is the test - except
 * hypot, which neither library rounds to the nearest every time:
 * there, within one unit in the last place.
 *
 * The numbers: the awkward ones by hand (zeros, the subnormals'
 * edges, halves, the largest double, infinities, NaN), then millions
 * of random bit patterns - every exponent equally likely, so the tiny,
 * the huge and the subnormal come up as often as 1.5 - and random
 * numbers near whole ones, where the rounding functions decide.
 *
 * Built and run by `make libc-test`. */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define D1(name) double taaj_##name(double); float taaj_##name##f(float);
#define D2(name) double taaj_##name(double, double); float taaj_##name##f(float, float);
D1(floor) D1(ceil) D1(trunc) D1(round) D1(rint) D1(nearbyint) D1(sqrt) D1(logb)
D2(fmod) D2(remainder) D2(fmin) D2(fmax) D2(fdim) D2(copysign) D2(nextafter) D2(hypot)
long taaj_lround(double);
long taaj_lrint(double);
double taaj_frexp(double, int *);
double taaj_modf(double, double *);
int taaj_ilogb(double);
int taaj___taaj_fpclassify(double);
int taaj___taaj_signbit(double);

/* What math.c calls, by its new name: the host's. */
double taaj_ldexp(double x, int n) { return ldexp(x, n); }
float taaj_ldexpf(float x, int n) { return ldexpf(x, n); }

static long checks, failures;

static uint64_t bits(double x)
{
    uint64_t u;
    memcpy(&u, &x, sizeof u);
    return u;
}

static uint32_t fbits(float x)
{
    uint32_t u;
    memcpy(&u, &x, sizeof u);
    return u;
}

static double from_bits(uint64_t u)
{
    double x;
    memcpy(&x, &u, sizeof x);
    return x;
}

/* Two answers are the same if their bits are, or both are NaN (which
 * NaN is not specified). */
static int same(double a, double b)
{
    return (isnan(a) && isnan(b)) || bits(a) == bits(b);
}

static int samef(float a, float b)
{
    return (isnan(a) && isnan(b)) || fbits(a) == fbits(b);
}

static void fail(const char *what, double x, double y, double got, double want)
{
    if (failures++ < 20)
        printf("FAIL: %s(%a, %a): %a, should be %a\n", what, x, y, got, want);
}

static void check1(double x)
{
#define C1(name)                                                         \
    do {                                                                 \
        checks++;                                                        \
        double got = taaj_##name(x), want = name(x);                     \
        if (!same(got, want))                                            \
            fail(#name, x, 0, got, want);                                \
        float fx = (float)x;                                             \
        checks++;                                                        \
        float gotf = taaj_##name##f(fx), wantf = name##f(fx);            \
        if (!samef(gotf, wantf))                                         \
            fail(#name "f", fx, 0, gotf, wantf);                         \
    } while (0)
    C1(floor);
    C1(ceil);
    C1(trunc);
    C1(round);
    C1(rint);
    C1(nearbyint);
    C1(sqrt);
    C1(logb);

    checks++;
    if (taaj___taaj_fpclassify(x) != fpclassify(x))
        fail("fpclassify", x, 0, taaj___taaj_fpclassify(x), fpclassify(x));
    checks++;
    if (taaj___taaj_signbit(x) != (signbit(x) != 0))
        fail("signbit", x, 0, taaj___taaj_signbit(x), signbit(x) != 0);

    checks++;
    int e1 = 0, e2 = 0;
    double f1 = taaj_frexp(x, &e1), f2 = frexp(x, &e2);
    if (!same(f1, f2) || (isfinite(x) && e1 != e2))
        fail("frexp", x, 0, f1, f2);

    checks++;
    double w1 = 0, w2 = 0;
    double m1 = taaj_modf(x, &w1), m2 = modf(x, &w2);
    if (!same(m1, m2) || !same(w1, w2))
        fail("modf", x, 0, m1, m2);

    checks++;
    if (taaj_ilogb(x) != ilogb(x))
        fail("ilogb", x, 0, taaj_ilogb(x), ilogb(x));

    /* lround and lrint only where the answer fits a long: outside, C
     * leaves it open. */
    if (fabs(x) < 9.2e18) {
        checks++;
        if (taaj_lround(x) != lround(x))
            fail("lround", x, 0, (double)taaj_lround(x), (double)lround(x));
        checks++;
        if (taaj_lrint(x) != lrint(x))
            fail("lrint", x, 0, (double)taaj_lrint(x), (double)lrint(x));
    }
}

static void check2(double x, double y)
{
#define C2(name)                                                         \
    do {                                                                 \
        checks++;                                                        \
        double got = taaj_##name(x, y), want = name(x, y);               \
        if (!same(got, want))                                            \
            fail(#name, x, y, got, want);                                \
        float fx = (float)x, fy = (float)y;                              \
        checks++;                                                        \
        float gotf = taaj_##name##f(fx, fy), wantf = name##f(fx, fy);    \
        if (!samef(gotf, wantf))                                         \
            fail(#name "f", fx, fy, gotf, wantf);                        \
    } while (0)
    C2(fmod);
    C2(remainder);
    C2(fmin);
    C2(fmax);
    C2(fdim);
    C2(copysign);
    C2(nextafter);

    /* hypot: within one unit in the last place of glibc's. */
    checks++;
    double got = taaj_hypot(x, y), want = hypot(x, y);
    if (!same(got, want)) {
        int64_t apart = (int64_t)(bits(got) - bits(want));
        if (isnan(got) || isnan(want) || apart > 1 || apart < -1)
            fail("hypot", x, y, got, want);
    }
}

/* xorshift: the same random numbers every run. */
static uint64_t state = 0x9e3779b97f4a7c15ULL;
static uint64_t next(void)
{
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    return state;
}

int main(void)
{
    static const double awkward[] = {
        0.0, -0.0, 0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 0.49999999999999994, -0.49999999999999994,
        1.0, -1.0, 3.0, 4503599627370495.5, 4503599627370496.0, 9007199254740993.0, 1e300, -1e300,
        4.9406564584124654e-324, -4.9406564584124654e-324, 2.2250738585072009e-308, 2.2250738585072014e-308,
        1.7976931348623157e308, -1.7976931348623157e308, 0.1, 7.0, -7.0, 2.0, 1e-310, 123456.789,
        INFINITY, -INFINITY, NAN,
    };
    size_t n = sizeof awkward / sizeof awkward[0];
    for (size_t i = 0; i < n; i++) {
        check1(awkward[i]);
        for (size_t j = 0; j < n; j++)
            check2(awkward[i], awkward[j]);
    }
    for (int i = 0; i < 2000000; i++) {
        double x = from_bits(next()), y = from_bits(next());
        check1(x);
        check2(x, y);
        /* Near a whole number, and with y within a few powers of two
         * of x, where fmod and remainder do the most work. */
        double near = (double)(int64_t)(next() % 2000001) / 2 - 500000.0 + (double)(next() % 3) * 1e-9;
        check1(near);
        check2(x, ldexp(x, (int)(next() % 40) - 20) * (1.0 + (double)(next() % 1000) / 1000));
    }
    printf("math: %ld checks against this machine's, %ld failed\n", checks, failures);
    return failures != 0;
}
