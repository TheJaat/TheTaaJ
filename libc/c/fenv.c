/* fenv.c - the floating-point environment (fenv.h), in the CPU's own
 * registers.
 *
 * x86_64 has two floating-point units, and both take part: SSE (MXCSR),
 * which does float and double, and the x87 (its control and status
 * words), which does long double. The rounding mode is set in both and
 * read from SSE; a flag is set if either has it, and cleared in both.
 *
 * AArch64 has one: FPCR holds the rounding mode (bits 22-23), FPSR the
 * flags (bits 0-4). */

#include <fenv.h>

#if defined(__x86_64__)

#define MXCSR_ROUND_SHIFT 3 /* MXCSR's rounding bits are the x87's << 3 */
#define ROUNDING          0xc00

static unsigned int get_mxcsr(void)
{
    unsigned int mxcsr;
    __asm__ volatile("stmxcsr %0" : "=m"(mxcsr));
    return mxcsr;
}

static void set_mxcsr(unsigned int mxcsr)
{
    __asm__ volatile("ldmxcsr %0" : : "m"(mxcsr));
}

static unsigned short get_x87_control(void)
{
    unsigned short control;
    __asm__ volatile("fnstcw %0" : "=m"(control));
    return control;
}

static void set_x87_control(unsigned short control)
{
    __asm__ volatile("fldcw %0" : : "m"(control));
}

static unsigned short get_x87_status(void)
{
    unsigned short status;
    __asm__ volatile("fnstsw %0" : "=m"(status));
    return status;
}

/* The x87's flags are cleared only all at once (fnclex), or by
 * rewriting its environment: fnstenv, change the status word, fldenv. */
static void clear_x87(int exceptions)
{
    struct {
        unsigned short control, unused1, status, unused2;
        unsigned int rest[5];
    } environment;
    __asm__ volatile("fnstenv %0" : "=m"(environment));
    environment.status &= (unsigned short)~exceptions;
    __asm__ volatile("fldenv %0" : : "m"(environment));
}

int feclearexcept(int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    clear_x87(exceptions);
    set_mxcsr(get_mxcsr() & ~(unsigned int)exceptions);
    return 0;
}

/* Raised in SSE: its flags are set as an instruction would set them.
 * (The exceptions are all masked, so nothing traps.) */
int feraiseexcept(int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    set_mxcsr(get_mxcsr() | (unsigned int)exceptions);
    return 0;
}

int fetestexcept(int exceptions)
{
    return (int)((get_mxcsr() | get_x87_status()) & (unsigned int)exceptions & FE_ALL_EXCEPT);
}

int fegetround(void)
{
    return (int)((get_mxcsr() >> MXCSR_ROUND_SHIFT) & ROUNDING);
}

int fesetround(int mode)
{
    if (mode & ~ROUNDING)
        return 1;
    set_x87_control((unsigned short)((get_x87_control() & ~ROUNDING) | mode));
    set_mxcsr((get_mxcsr() & ~(ROUNDING << MXCSR_ROUND_SHIFT)) | ((unsigned int)mode << MXCSR_ROUND_SHIFT));
    return 0;
}

int fegetenv(fenv_t *environment)
{
    environment->control = get_x87_control();
    environment->status = get_x87_status() & FE_ALL_EXCEPT;
    environment->mxcsr = get_mxcsr();
    return 0;
}

int fesetenv(const fenv_t *environment)
{
    /* What a program starts with: every exception masked, to nearest,
     * no flags; the x87 at 64-bit precision (control word 0x37f). */
    fenv_t start = { 0x37f, 0, 0x1f80 };
    if (environment == FE_DFL_ENV)
        environment = &start;
    set_x87_control(environment->control);
    clear_x87(FE_ALL_EXCEPT & ~environment->status);
    if (environment->status & FE_ALL_EXCEPT) {
        /* fldenv can set the x87's flags too, but a set flag with its
         * exception masked is all a flag is: SSE holds it as well. */
        set_mxcsr(environment->mxcsr | (environment->status & FE_ALL_EXCEPT));
    } else {
        set_mxcsr(environment->mxcsr);
    }
    return 0;
}

int fegetexceptflag(fexcept_t *flags, int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    *flags = (fexcept_t)fetestexcept(exceptions);
    return 0;
}

#elif defined(__aarch64__)

#define ROUNDING 0xc00000

static unsigned long get_fpcr(void)
{
    unsigned long fpcr;
    __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
    return fpcr;
}

static void set_fpcr(unsigned long fpcr)
{
    __asm__ volatile("msr fpcr, %0" : : "r"(fpcr));
}

static unsigned long get_fpsr(void)
{
    unsigned long fpsr;
    __asm__ volatile("mrs %0, fpsr" : "=r"(fpsr));
    return fpsr;
}

static void set_fpsr(unsigned long fpsr)
{
    __asm__ volatile("msr fpsr, %0" : : "r"(fpsr));
}

int feclearexcept(int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    set_fpsr(get_fpsr() & ~(unsigned long)exceptions);
    return 0;
}

/* The trap bits in FPCR are all off, so this only sets flags. */
int feraiseexcept(int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    set_fpsr(get_fpsr() | (unsigned long)exceptions);
    return 0;
}

int fetestexcept(int exceptions)
{
    return (int)(get_fpsr() & (unsigned long)exceptions & FE_ALL_EXCEPT);
}

int fegetround(void)
{
    return (int)(get_fpcr() & ROUNDING);
}

int fesetround(int mode)
{
    if (mode & ~ROUNDING)
        return 1;
    set_fpcr((get_fpcr() & ~(unsigned long)ROUNDING) | (unsigned long)mode);
    return 0;
}

int fegetenv(fenv_t *environment)
{
    environment->fpcr = (unsigned int)get_fpcr();
    environment->fpsr = (unsigned int)get_fpsr();
    return 0;
}

int fesetenv(const fenv_t *environment)
{
    /* What a thread starts with (the kernel zeroes both). */
    fenv_t start = { 0, 0 };
    if (environment == FE_DFL_ENV)
        environment = &start;
    set_fpcr(environment->fpcr);
    set_fpsr(environment->fpsr);
    return 0;
}

int fegetexceptflag(fexcept_t *flags, int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    *flags = (fexcept_t)fetestexcept(exceptions);
    return 0;
}

#endif

/* The same on both: in terms of the above. */

int fesetexceptflag(const fexcept_t *flags, int exceptions)
{
    if (exceptions & ~FE_ALL_EXCEPT)
        return 1;
    feclearexcept(exceptions & ~(int)*flags);
    feraiseexcept(exceptions & (int)*flags);
    return 0;
}

int feholdexcept(fenv_t *environment)
{
    fegetenv(environment);
    feclearexcept(FE_ALL_EXCEPT);
    return 0;
}

int feupdateenv(const fenv_t *environment)
{
    int raised = fetestexcept(FE_ALL_EXCEPT);
    fesetenv(environment);
    feraiseexcept(raised);
    return 0;
}
