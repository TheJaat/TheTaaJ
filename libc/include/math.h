/* math.h - floating-point mathematics.
 *
 * Everything C99 names is declared here, because C++'s <cmath> brings
 * each one into namespace std and expects to find it. Not everything is
 * there yet to link with. What libc/c/math.c and float.c have:
 *
 *   fabs, copysign, floor, ceil, trunc, round, lround, llround, rint,
 *   nearbyint, lrint, llrint, fmod, remainder, frexp, ldexp, scalbn,
 *   modf, fmin, fmax, fdim, nan, sqrt, hypot, logb, ilogb
 *
 * each for double and float (the float ones go through double, which is
 * exact for all of these: a float fits in a double with room to spare)
 * and ldexpl and fabsl for long double. The approximations, from musl
 * (libc/c/libm): sin, cos, tan, asin, acos, atan, atan2, sinh, cosh,
 * tanh, asinh, acosh, atanh, exp, exp2, expm1, log, log2, log10, log1p,
 * pow and cbrt, for double and float; each within one unit in the last
 * place of the true value. erf, erfc, tgamma, lgamma and the long double
 * versions are declared but not there: a program that calls one does
 * not link.
 *
 * The classification macros (isnan, fpclassify, signbit, ...) are the
 * compiler's own built-ins with clang; TCC has none, and calls the
 * library. */
#ifndef _MATH_H
#define _MATH_H

#ifdef __cplusplus
extern "C" {
#endif

#define HUGE_VAL __builtin_huge_val()
#define HUGE_VALF __builtin_huge_valf()
#define HUGE_VALL __builtin_huge_vall()
#ifdef __TINYC__
/* TCC has no __builtin_inff; its own predefinitions make
 * __builtin_huge_valf a float constant too big to be anything but
 * infinite. */
#define INFINITY __builtin_huge_valf()
#else
#define INFINITY __builtin_inff()
#endif
#define NAN __builtin_nanf("")

/* What fpclassify says a number is. */
#define FP_NAN       0
#define FP_INFINITE  1
#define FP_ZERO      2
#define FP_SUBNORMAL 3
#define FP_NORMAL    4

/* The types arithmetic is done in: no wider than the operands, on both
 * CPUs (SSE and AArch64's registers each hold a float as a float). */
typedef float float_t;
typedef double double_t;

/* How a function reports a domain or range error: by errno would be
 * MATH_ERRNO (1), by the floating-point flags MATH_ERREXCEPT (2).
 * These functions report neither yet - errno is not set, and nothing
 * here reads or sets the flags - so this says 0. */
#define MATH_ERRNO     1
#define MATH_ERREXCEPT 2
#define math_errhandling 0

#define FP_ILOGB0   (-2147483647 - 1)
#define FP_ILOGBNAN (-2147483647 - 1)

/* Constants from POSIX, not C: pi, e and their kin, to double's
 * precision. */
#define M_E        2.7182818284590452354
#define M_LOG2E    1.4426950408889634074
#define M_LOG10E   0.43429448190325182765
#define M_LN2      0.69314718055994530942
#define M_LN10     2.30258509299404568402
#define M_PI       3.14159265358979323846
#define M_PI_2     1.57079632679489661923
#define M_PI_4     0.78539816339744830962
#define M_1_PI     0.31830988618379067154
#define M_2_PI     0.63661977236758134308
#define M_2_SQRTPI 1.12837916709551257390
#define M_SQRT2    1.41421356237309504880
#define M_SQRT1_2  0.70710678118654752440

#if defined(__clang__) || defined(__GNUC__)
#define fpclassify(x) __builtin_fpclassify(FP_NAN, FP_INFINITE, FP_NORMAL, FP_SUBNORMAL, FP_ZERO, x)
#define isfinite(x) __builtin_isfinite(x)
#define isinf(x) __builtin_isinf(x)
#define isnan(x) __builtin_isnan(x)
#define isnormal(x) __builtin_isnormal(x)
#define signbit(x) __builtin_signbit(x)
#define isgreater(x, y) __builtin_isgreater(x, y)
#define isgreaterequal(x, y) __builtin_isgreaterequal(x, y)
#define isless(x, y) __builtin_isless(x, y)
#define islessequal(x, y) __builtin_islessequal(x, y)
#define islessgreater(x, y) __builtin_islessgreater(x, y)
#define isunordered(x, y) __builtin_isunordered(x, y)
#else
/* Without built-ins (TCC): functions of a double. A float converts to
 * a double exactly, keeping its class and its sign, so one function
 * does for both. */
int __taaj_fpclassify(double x);
int __taaj_signbit(double x);
#define fpclassify(x) __taaj_fpclassify(x)
#define isfinite(x) (__taaj_fpclassify(x) >= FP_ZERO)
#define isinf(x) (__taaj_fpclassify(x) == FP_INFINITE)
#define isnan(x) (__taaj_fpclassify(x) == FP_NAN)
#define isnormal(x) (__taaj_fpclassify(x) == FP_NORMAL)
#define signbit(x) __taaj_signbit(x)
#define isunordered(x, y) (isnan(x) || isnan(y))
#define isgreater(x, y) (!isunordered(x, y) && (x) > (y))
#define isgreaterequal(x, y) (!isunordered(x, y) && (x) >= (y))
#define isless(x, y) (!isunordered(x, y) && (x) < (y))
#define islessequal(x, y) (!isunordered(x, y) && (x) <= (y))
#define islessgreater(x, y) (!isunordered(x, y) && ((x) < (y) || (x) > (y)))
#endif

/* Each function three times: double, float (f), long double (l). */
#define _TAAJ_MATH1(name) double name(double); float name##f(float); long double name##l(long double);
#define _TAAJ_MATH2(name) double name(double, double); float name##f(float, float); \
    long double name##l(long double, long double);

/* Exact - math.c and float.c. */
_TAAJ_MATH1(fabs)
_TAAJ_MATH2(copysign)
_TAAJ_MATH1(floor)
_TAAJ_MATH1(ceil)
_TAAJ_MATH1(trunc)
_TAAJ_MATH1(round)
_TAAJ_MATH1(rint)
_TAAJ_MATH1(nearbyint)
_TAAJ_MATH2(fmod)
_TAAJ_MATH2(remainder)
_TAAJ_MATH2(fmin)
_TAAJ_MATH2(fmax)
_TAAJ_MATH2(fdim)
_TAAJ_MATH1(sqrt)
_TAAJ_MATH2(hypot)
_TAAJ_MATH1(logb)
long lround(double x);
long lroundf(float x);
long lroundl(long double x);
long long llround(double x);
long long llroundf(float x);
long long llroundl(long double x);
long lrint(double x);
long lrintf(float x);
long lrintl(long double x);
long long llrint(double x);
long long llrintf(float x);
long long llrintl(long double x);
double frexp(double x, int *exponent);
float frexpf(float x, int *exponent);
long double frexpl(long double x, int *exponent);
double ldexp(double x, int n);
float ldexpf(float x, int n);
long double ldexpl(long double x, int n);
double scalbn(double x, int n);
float scalbnf(float x, int n);
long double scalbnl(long double x, int n);
double scalbln(double x, long n);
float scalblnf(float x, long n);
long double scalblnl(long double x, long n);
int ilogb(double x);
int ilogbf(float x);
int ilogbl(long double x);
double modf(double x, double *whole);
float modff(float x, float *whole);
long double modfl(long double x, long double *whole);
double nan(const char *tag);
float nanf(const char *tag);
long double nanl(const char *tag);
double nextafter(double x, double y);
float nextafterf(float x, float y);
long double nextafterl(long double x, long double y);
double nexttoward(double x, long double y);
float nexttowardf(float x, long double y);
long double nexttowardl(long double x, long double y);
double remquo(double x, double y, int *quotient);
float remquof(float x, float y, int *quotient);
long double remquol(long double x, long double y, int *quotient);
double fma(double x, double y, double z);
float fmaf(float x, float y, float z);
long double fmal(long double x, long double y, long double z);

/* Approximations (libc/c/libm). The last four, and every long double
 * one, are declared only. */
_TAAJ_MATH1(sin)
_TAAJ_MATH1(cos)
_TAAJ_MATH1(tan)
_TAAJ_MATH1(asin)
_TAAJ_MATH1(acos)
_TAAJ_MATH1(atan)
_TAAJ_MATH2(atan2)
_TAAJ_MATH1(sinh)
_TAAJ_MATH1(cosh)
_TAAJ_MATH1(tanh)
_TAAJ_MATH1(asinh)
_TAAJ_MATH1(acosh)
_TAAJ_MATH1(atanh)
_TAAJ_MATH1(exp)
_TAAJ_MATH1(exp2)
_TAAJ_MATH1(expm1)
_TAAJ_MATH1(log)
_TAAJ_MATH1(log10)
_TAAJ_MATH1(log2)
_TAAJ_MATH1(log1p)
_TAAJ_MATH2(pow)
_TAAJ_MATH1(cbrt)
_TAAJ_MATH1(erf)
_TAAJ_MATH1(erfc)
_TAAJ_MATH1(tgamma)
_TAAJ_MATH1(lgamma)

#undef _TAAJ_MATH1
#undef _TAAJ_MATH2

/* The absolute value, in C: clang does it itself, in one instruction.
 * (Not in C++, where fabs must stay a function: <cmath> overloads it.) */
#if (defined(__clang__) || defined(__GNUC__)) && !defined(__cplusplus)
#define fabs(x) __builtin_fabs(x)
#define fabsf(x) __builtin_fabsf(x)
#define fabsl(x) __builtin_fabsl(x)
#endif

#ifdef __cplusplus
}
#endif

#endif
