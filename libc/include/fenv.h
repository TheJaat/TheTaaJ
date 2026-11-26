/* fenv.h - the floating-point environment (C99): which way results
 * are rounded, and the exceptions that have happened (a division by
 * zero, an overflow, an inexact result...). Each thread has its own:
 * the kernel saves the CPU's control and status registers with the
 * rest of a thread's (x86_64: MXCSR and the x87 control and status
 * words; AArch64: FPCR and FPSR). libc/c/fenv.c.
 *
 * Exceptions only set flags here: nothing turns them into SIGFPE
 * (feenableexcept is not provided). The values are Linux's. */
#ifndef _FENV_H
#define _FENV_H

#ifdef __cplusplus
extern "C" {
#endif

#if defined(__x86_64__)

/* The flags, as the x87 status word and MXCSR have them. */
#define FE_INVALID    0x01
#define FE_DIVBYZERO  0x04
#define FE_OVERFLOW   0x08
#define FE_UNDERFLOW  0x10
#define FE_INEXACT    0x20
#define FE_ALL_EXCEPT 0x3d

/* The rounding modes, as the x87 control word has them (MXCSR's are
 * the same, three bits higher). */
#define FE_TONEAREST  0x000
#define FE_DOWNWARD   0x400
#define FE_UPWARD     0x800
#define FE_TOWARDZERO 0xc00

typedef unsigned short fexcept_t;
typedef struct {
    unsigned short control; /* the x87 control word */
    unsigned short status;  /* the x87 status word's flags */
    unsigned int mxcsr;     /* SSE's control and flags */
} fenv_t;

#elif defined(__aarch64__)

/* The flags, as FPSR has them. */
#define FE_INVALID    0x01
#define FE_DIVBYZERO  0x02
#define FE_OVERFLOW   0x04
#define FE_UNDERFLOW  0x08
#define FE_INEXACT    0x10
#define FE_ALL_EXCEPT 0x1f

/* The rounding modes, as FPCR's RMode field has them. */
#define FE_TONEAREST  0x000000
#define FE_UPWARD     0x400000
#define FE_DOWNWARD   0x800000
#define FE_TOWARDZERO 0xc00000

typedef unsigned int fexcept_t;
typedef struct {
    unsigned int fpcr;
    unsigned int fpsr;
} fenv_t;

#else
#error "fenv.h: no floating-point environment for this CPU"
#endif

/* The environment a program starts in: to nearest, no flags. */
#define FE_DFL_ENV ((const fenv_t *)-1)

/* Each returns 0 for success, as C says (fegetround the mode, and
 * fetestexcept the flags that are set). An exception or a mode that is
 * not one of the above is refused with a nonzero result. */
int feclearexcept(int exceptions);
int feraiseexcept(int exceptions);
int fetestexcept(int exceptions);
int fegetexceptflag(fexcept_t *flags, int exceptions);
int fesetexceptflag(const fexcept_t *flags, int exceptions);
int fegetround(void);
int fesetround(int mode);
int fegetenv(fenv_t *environment);
int fesetenv(const fenv_t *environment);
/* feholdexcept: saves the environment, then clears the flags. */
int feholdexcept(fenv_t *environment);
/* feupdateenv: installs `environment`, then raises the flags that were
 * set before it. */
int feupdateenv(const fenv_t *environment);

#ifdef __cplusplus
}
#endif

#endif
