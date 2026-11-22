/* math.h - so far, only what a C compiler running on TheTaaJ needs:
 * scaling by powers of two, and the absolute value. (libc/c/float.c) */
#ifndef _MATH_H
#define _MATH_H

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

/* x times 2 to the power n: exact, unless the result is too big (it is
 * then infinite) or too small to be held with all its bits. */
double ldexp(double x, int n);
float ldexpf(float x, int n);
long double ldexpl(long double x, int n);

/* The absolute value. Clang does these itself, in one instruction;
 * other compilers (TCC) call the C library's. */
double fabs(double x);
float fabsf(float x);
long double fabsl(long double x);
#if defined(__clang__) || defined(__GNUC__)
#define fabs(x) __builtin_fabs(x)
#define fabsf(x) __builtin_fabsf(x)
#define fabsl(x) __builtin_fabsl(x)
#endif

#endif
