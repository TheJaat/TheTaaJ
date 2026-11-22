/* libm_test.c - the library's approximate mathematics (libc/c/libm:
 * sin, exp, log, pow and the rest, from musl), on this machine.
 *
 * Runs on the host, not in TheTaaJ, as math_test.c does: libm's files
 * and math.c are compiled for the host as they are for TheTaaJ, joined
 * into one object, and every name defined in it gets "taaj_" in front.
 * So each function can be given the same numbers as the host's, and the
 * answers compared.
 *
 * What they are compared with is not glibc's double functions, which
 * are not always closer than musl's: glibc's cbrt is up to 2 units in
 * the last place (ulps) off, where musl's is right. It is glibc's long
 * double functions (on the PC, 64 bits of fraction to a double's 52),
 * rounded to a double: the true value rounded to the nearest double,
 * but for the rare number that lies almost exactly between two.
 *
 * musl promises an error below one ulp, and is nearly always exact. So
 * the rule is: the same NaN-ness, the same infinities and zeros (with
 * their signs), and otherwise at most one ulp from the nearest double.
 * How many answers are exactly the nearest is counted and printed too:
 * it should be nearly all of them.
 *
 * The numbers: the awkward ones by hand, random bit patterns (every
 * exponent equally likely), and random numbers in the range where each
 * function does its real work (near the multiples of pi/2 for sin and
 * cos, around 0 and 700 for exp, around 1 for log, ...).
 *
 * fma is the exception: glibc's fma is exact, as musl's is, so there the
 * two must agree to the bit, and are compared directly.
 *
 * Built and run by `make libc-test`. */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef double (*one)(double);
typedef long double (*long_one)(long double);

#define D1(name) double taaj_##name(double);
#define D2(name) double taaj_##name(double, double);
D1(sin) D1(cos) D1(tan) D1(asin) D1(acos) D1(atan) D1(sinh) D1(cosh) D1(tanh) D1(exp) D1(exp2) D1(expm1)
D1(log) D1(log2) D1(log10) D1(log1p) D1(cbrt) D1(asinh) D1(acosh) D1(atanh)
D2(atan2) D2(pow)
double taaj_fma(double, double, double);
float taaj_sinf(float);
float taaj_powf(float, float);

static long checks, failures, exact;

static uint64_t bits(double x)
{
    uint64_t u;
    memcpy(&u, &x, sizeof u);
    return u;
}

static double from_bits(uint64_t u)
{
    double x;
    memcpy(&x, &u, sizeof x);
    return x;
}

/* How far apart two finite doubles of the same sign are, in ulps: the
 * distance between their bit patterns, which for doubles of one sign
 * count up as the numbers do. */
static uint64_t ulps(double a, double b)
{
    int64_t x = (int64_t)bits(a), y = (int64_t)bits(b);
    if (x < 0)
        x = (int64_t)0x8000000000000000ULL - x;
    if (y < 0)
        y = (int64_t)0x8000000000000000ULL - y;
    return x > y ? (uint64_t)(x - y) : (uint64_t)(y - x);
}

/* How far off each function may be, in ulps from the nearest double,
 * and how far off it was at worst. The hyperbolic functions and their
 * inverses, which musl builds from exp, expm1, log and log1p (whose own
 * errors then add up), are within 2 ulps, not 1: musl's acosh.c says
 * "up to 2ulp error", and glibc's own table of errors says 2 for sinh,
 * cosh and tanh on the PC. */
static struct { const char *name; uint64_t limit, worst; } limits[] = {
    {"sin", 1, 0}, {"cos", 1, 0}, {"tan", 1, 0}, {"asin", 1, 0}, {"acos", 1, 0}, {"atan", 1, 0},
    {"atan2", 1, 0}, {"sinh", 2, 0}, {"cosh", 2, 0}, {"tanh", 2, 0}, {"exp", 1, 0}, {"exp2", 1, 0},
    {"expm1", 1, 0}, {"log", 1, 0}, {"log2", 1, 0}, {"log10", 1, 0}, {"log1p", 1, 0}, {"cbrt", 1, 0},
    {"asinh", 2, 0}, {"acosh", 2, 0}, {"atanh", 2, 0}, {"pow", 1, 0},
};
#define LIMITS (sizeof limits / sizeof limits[0])

static size_t limit_of(const char *name)
{
    size_t i = 0;
    while (strcmp(limits[i].name, name) != 0)
        i++;
    return i;
}

/* How far `got` is from `want`, in ulps; or UINT64_MAX when one is a
 * NaN, an infinity or a zero and the other is not the same. */
static uint64_t distance(double got, double want)
{
    if (isnan(got) || isnan(want))
        return isnan(got) && isnan(want) ? 0 : UINT64_MAX;
    if (isinf(got) || isinf(want) || got == 0 || want == 0)
        return bits(got) == bits(want) ? 0 : UINT64_MAX;
    if ((got < 0) != (want < 0))
        return UINT64_MAX;
    return ulps(got, want);
}

static void compare(const char *name, double x, double y, int has_y, double got, double want)
{
    checks++;
    uint64_t off = distance(got, want);
    if (off == 0) {
        exact++;
        return;
    }
    size_t i = limit_of(name);
    if (off > limits[i].worst)
        limits[i].worst = off;
    if (off <= limits[i].limit)
        return;
    if (failures++ < 20) {
        if (has_y)
            printf("libm: %s(%a, %a) = %a, the nearest double is %a\n", name, x, y, got, want);
        else
            printf("libm: %s(%a) = %a, the nearest double is %a\n", name, x, got, want);
    }
}

static const struct { const char *name; one taaj; long_one host; } ones[] = {
    {"sin", taaj_sin, sinl}, {"cos", taaj_cos, cosl}, {"tan", taaj_tan, tanl},
    {"asin", taaj_asin, asinl}, {"acos", taaj_acos, acosl}, {"atan", taaj_atan, atanl},
    {"sinh", taaj_sinh, sinhl}, {"cosh", taaj_cosh, coshl}, {"tanh", taaj_tanh, tanhl},
    {"exp", taaj_exp, expl}, {"exp2", taaj_exp2, exp2l}, {"expm1", taaj_expm1, expm1l},
    {"log", taaj_log, logl}, {"log2", taaj_log2, log2l}, {"log10", taaj_log10, log10l},
    {"log1p", taaj_log1p, log1pl}, {"cbrt", taaj_cbrt, cbrtl},
    {"asinh", taaj_asinh, asinhl}, {"acosh", taaj_acosh, acoshl}, {"atanh", taaj_atanh, atanhl},
};
#define ONES (sizeof ones / sizeof ones[0])

static void check1(double x)
{
    for (size_t i = 0; i < ONES; i++)
        compare(ones[i].name, x, 0, 0, ones[i].taaj(x), (double)ones[i].host(x));
}

static void check2(double x, double y)
{
    compare("atan2", x, y, 1, taaj_atan2(x, y), (double)atan2l(x, y));
    compare("pow", x, y, 1, taaj_pow(x, y), (double)powl(x, y));
}

static uint64_t state = 0x9e3779b97f4a7c15ULL;
static uint64_t next(void)
{
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    return state;
}

/* A random double in [low, high). */
static double between(double low, double high)
{
    return low + (high - low) * ((double)(next() >> 11) / 9007199254740992.0);
}

int main(void)
{
    static const double awkward[] = {
        0.0, -0.0, 0.5, -0.5, 1.0, -1.0, 2.0, -2.0, 3.0, 0.1, 10.0, 1e-300, -1e-300, 1e300, -1e300,
        4.9406564584124654e-324, 2.2250738585072014e-308, 1.7976931348623157e308, -1.7976931348623157e308,
        709.782712893384, 709.79, -745.1332191019411, -745.14, 710.4758600739439, 1.5707963267948966,
        3.141592653589793, 6.283185307179586, 1e22, 0x1p1023, 0x1.fffffffffffffp-1, 0x1.0000000000001p0,
        INFINITY, -INFINITY, NAN,
    };
    size_t n = sizeof awkward / sizeof awkward[0];
    for (size_t i = 0; i < n; i++) {
        check1(awkward[i]);
        for (size_t j = 0; j < n; j++)
            check2(awkward[i], awkward[j]);
    }
    for (int i = 0; i < 300000; i++) {
        /* Any double at all. */
        check1(from_bits(next()));
        check2(from_bits(next()), from_bits(next()));
        /* Where the functions do their work. */
        check1(between(-10, 10));
        check1(between(-1, 1));
        check1(between(-800, 800));
        check1(between(0.5, 2));
        check1(between(1, 1e6));
        /* sin, cos, tan near a multiple of pi/2, where the reduction
         * of the argument must be exact. */
        double k = (double)(int64_t)(next() % 2000001) - 1000000.0;
        check1(k * 1.5707963267948966 + between(-1e-9, 1e-9));
        /* pow: a base near 1 to a large power, and the ordinary case. */
        check2(between(0.9, 1.1), between(-2000, 2000));
        check2(between(0, 100), between(-30, 30));
        check2(-between(0, 100), (double)(int64_t)(next() % 61) - 30.0);
    }
    /* fma: x*y+z rounded once. glibc's fma is exact too, so the two must
     * agree to the bit; a product and a sum that nearly cancel, and the
     * subnormal results, are where a careless one goes wrong. */
    for (int i = 0; i < 1000000; i++) {
        double x = from_bits(next()), y = from_bits(next()), z = from_bits(next());
        if (i % 4 == 1) {
            x = between(-2, 2), y = between(-2, 2);
            z = -x * y * (1 + between(-1e-15, 1e-15));
        } else if (i % 4 == 2) {
            x = between(-1, 1) * 0x1p-520, y = between(-1, 1) * 0x1p-520, z = between(-1, 1) * 0x1p-1030;
        } else if (i % 4 == 3) {
            x = between(-1e10, 1e10), y = between(-1e10, 1e10), z = between(-1e20, 1e20);
        }
        checks++;
        double got = taaj_fma(x, y, z), want = fma(x, y, z);
        if (bits(got) == bits(want) || (isnan(got) && isnan(want))) {
            exact++;
        } else if (failures++ < 20) {
            printf("libm: fma(%a, %a, %a) = %a, not %a\n", x, y, z, got, want);
        }
    }
    /* The float functions are the double ones, rounded once more. */
    for (int i = 0; i < 100000; i++) {
        float x = (float)between(-100, 100), y = (float)between(-10, 10);
        checks += 2;
        if (taaj_sinf(x) != (float)sinl(x) || (fabsf(x) < 10 && taaj_powf(fabsf(x), y) != (float)powl(fabsf(x), y))) {
            if (failures++ < 20)
                printf("libm: sinf or powf of %a differs\n", (double)x);
        }
    }
    printf("libm: %ld checks against this machine's long double, %ld the nearest double, %ld too far off\n", checks,
           exact, failures);
    printf("libm: the most ulps off:");
    for (size_t i = 0; i < LIMITS; i++) {
        if (limits[i].worst == UINT64_MAX)
            printf(" %s (a NaN, infinity or zero wrong)", limits[i].name);
        else
            printf(" %s %llu", limits[i].name, (unsigned long long)limits[i].worst);
    }
    printf("\n");
    return failures != 0;
}
