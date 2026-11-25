/* setjmp.h - setjmp(buffer) marks a place and returns 0; a later
 * longjmp(buffer, value) from any function called since then jumps
 * back there, so that setjmp returns again, with value (1 if value is
 * 0). How C programs get out of deep calls at once - on an error, say.
 * Local variables of the function that called setjmp, changed after
 * it, should be volatile if they are to be read after the jump.
 * (libc/c/setjmp.c) */
#ifndef _SETJMP_H
#define _SETJMP_H

#ifdef __cplusplus
extern "C" {
#endif

/* C11's _Noreturn is C++'s [[noreturn]]. */
#if defined(__cplusplus) && !defined(_Noreturn)
#define _Noreturn [[noreturn]]
#endif

/* What is kept: the registers a call must preserve, and where to go
 * back to - 8 of them on x86_64; 22 on AArch64, which preserves more. */
#if defined(__aarch64__)
typedef long jmp_buf[22];
#else
typedef long jmp_buf[8];
#endif

int setjmp(jmp_buf buffer);
_Noreturn void longjmp(jmp_buf buffer, int value);

/* sigsetjmp(buffer, save) is setjmp that, if save is not 0, also keeps
 * the blocked signals; siglongjmp(buffer, value) puts them back before
 * it jumps. The way out of a signal handler: the handler's signal is
 * blocked while it runs, and a jump out of it with plain longjmp leaves
 * it blocked. A macro, because setjmp must be called by the function
 * that is to be returned to, not by one in between. */
typedef struct {
    jmp_buf jump;
    int saved;
    unsigned long mask;
} sigjmp_buf[1];

void __taaj_sigjmp_save(unsigned long *mask);
#define sigsetjmp(buffer, save) \
    ((buffer)->saved = (save) != 0, (save) ? __taaj_sigjmp_save(&(buffer)->mask) : (void)0, setjmp((buffer)->jump))
_Noreturn void siglongjmp(sigjmp_buf buffer, int value);

#ifdef __cplusplus
}
#endif

#endif
