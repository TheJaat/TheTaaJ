/* setjmp.h - setjmp(buffer) marks a place and returns 0; a later
 * longjmp(buffer, value) from any function called since then jumps
 * back there, so that setjmp returns again, with value (1 if value is
 * 0). How C programs get out of deep calls at once - on an error, say.
 * Local variables of the function that called setjmp, changed after
 * it, should be volatile if they are to be read after the jump.
 * (libc/c/setjmp.c) */
#ifndef _SETJMP_H
#define _SETJMP_H

/* What is kept: the registers a call must preserve, and where to go
 * back to - 8 of them on x86_64; 22 on AArch64, which preserves more. */
#if defined(__aarch64__)
typedef long jmp_buf[22];
#else
typedef long jmp_buf[8];
#endif

int setjmp(jmp_buf buffer);
_Noreturn void longjmp(jmp_buf buffer, int value);

#endif
