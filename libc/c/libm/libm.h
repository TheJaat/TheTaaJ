/* libm.h - what musl's math files expect of musl's internal libm.h,
 * for TheTaaJ (written for TheTaaJ; musl's own is not copied, since
 * most of it is for long double and the functions not taken).
 *
 * The files are compiled as one, by ../libm.c, so their helpers are
 * `static`: musl's `hidden` (visible across musl's files, not outside
 * the library) becomes "visible in this one file". */

#ifndef _TAAJ_LIBM_H
#define _TAAJ_LIBM_H

#include <float.h>
#include <math.h>
#include <stdint.h>

#define hidden

/* Rounding modes other than to-nearest are honoured where musl's code
 * checks for them; signalling NaNs are not told apart. */
#define WANT_ROUNDING 1
#define WANT_SNAN 0
#define issignaling_inline(x) 0
#define issignalingf_inline(x) 0
#define TOINT_INTRINSICS 0

#define predict_true(x) (x)
#define predict_false(x) (x)

static inline double eval_as_double(double x)
{
    double y = x;
    return y;
}

static inline float eval_as_float(float x)
{
    float y = x;
    return y;
}

/* A value the compiler must treat as unknown, and a computation it must
 * do even though the result is not used (for the floating-point flags
 * it raises: overflow, inexact). */
static inline double fp_barrier(double x)
{
    volatile double y = x;
    return y;
}

static inline float fp_barrierf(float x)
{
    volatile float y = x;
    return y;
}

static inline void fp_force_eval(double x)
{
    volatile double y;
    y = x;
    (void)y;
}

static inline void fp_force_evalf(float x)
{
    volatile float y;
    y = x;
    (void)y;
}

#define FORCE_EVAL(x) do {                    \
    if (sizeof(x) == sizeof(float))           \
        fp_force_evalf(x);                    \
    else                                      \
        fp_force_eval(x);                     \
} while (0)

/* A number's bits, and a number from bits. */
static inline uint32_t asuint(float f) { union { float f; uint32_t i; } u; u.f = f; return u.i; }
static inline float asfloat(uint32_t i) { union { uint32_t i; float f; } u; u.i = i; return u.f; }
static inline uint64_t asuint64(double f) { union { double f; uint64_t i; } u; u.f = f; return u.i; }
static inline double asdouble(uint64_t i) { union { uint64_t i; double f; } u; u.i = i; return u.f; }

#define EXTRACT_WORDS(hi, lo, d) do { uint64_t __u = asuint64(d); (hi) = __u >> 32; (lo) = (uint32_t)__u; } while (0)
#define GET_HIGH_WORD(hi, d) do { (hi) = asuint64(d) >> 32; } while (0)
#define GET_LOW_WORD(lo, d) do { (lo) = (uint32_t)asuint64(d); } while (0)
#define INSERT_WORDS(d, hi, lo) do { (d) = asdouble(((uint64_t)(hi) << 32) | (uint32_t)(lo)); } while (0)
#define SET_HIGH_WORD(d, hi) INSERT_WORDS(d, hi, (uint32_t)asuint64(d))
#define SET_LOW_WORD(d, lo) INSERT_WORDS(d, asuint64(d) >> 32, lo)
#define GET_FLOAT_WORD(w, d) do { (w) = asuint(d); } while (0)
#define SET_FLOAT_WORD(d, w) do { (d) = asfloat(w); } while (0)

/* How many zero bits lead x, which is not 0 (fma.c). musl takes it from
 * its atomic.h, which may use an instruction; this is the plain C, so
 * that TCC can compile it too: halving the range six times. */
static inline int a_clz_64(uint64_t x)
{
    int n = 0;
    if (!(x >> 32)) { n += 32; x <<= 32; }
    if (!(x >> 48)) { n += 16; x <<= 16; }
    if (!(x >> 56)) { n += 8; x <<= 8; }
    if (!(x >> 60)) { n += 4; x <<= 4; }
    if (!(x >> 62)) { n += 2; x <<= 2; }
    if (!(x >> 63)) { n += 1; }
    return n;
}

hidden int __rem_pio2_large(double *, double *, int, int, int);
hidden int __rem_pio2(double, double *);
hidden double __sin(double, double, int);
hidden double __cos(double, double);
hidden double __tan(double, double, int);
hidden double __expo2(double, double);

hidden double __math_xflow(uint32_t, double);
hidden double __math_uflow(uint32_t);
hidden double __math_oflow(uint32_t);
hidden double __math_divzero(uint32_t);
hidden double __math_invalid(double);

#endif
