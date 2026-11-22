/* setjmp.c - setjmp and longjmp: a jump from deep inside nested calls
 * straight back to a place an outer function marked.
 *
 * setjmp(buffer) keeps what a function call must preserve - the
 * registers the calling convention calls "callee-saved", the stack
 * pointer, and where setjmp was called from - and returns 0. Later,
 * longjmp(buffer, value) puts all of that back and jumps to that
 * return address, so it is as if setjmp had returned a second time,
 * now with `value` (1 if `value` is 0: setjmp's 0 means "the first
 * time"). Everything the calls in between had on the stack is simply
 * abandoned. A compiler uses this for error recovery: TCC calls
 * longjmp when it finds an error, to get back to its top level.
 *
 * The other registers need no keeping: at a call, the caller has
 * already given them up. It cannot be written in C - C has no way to
 * name the stack pointer - so it is assembly, one version for each
 * machine. */

#if defined(__x86_64__)

/* jmp_buf: rbx, rbp, r12, r13, r14, r15, the stack pointer as it will
 * be after setjmp returns, and the return address.
 *
 * In AT&T syntax - source first, then destination; registers with %,
 * constants with $, and 8(%rdi) for [rdi + 8] - because it is the one
 * both clang and TCC's own assembler read. */
__asm__(
    ".text\n"
    ".global setjmp\n"
    ".type setjmp, @function\n"
    "setjmp:\n"
    "    movq %rbx, (%rdi)\n"
    "    movq %rbp, 8(%rdi)\n"
    "    movq %r12, 16(%rdi)\n"
    "    movq %r13, 24(%rdi)\n"
    "    movq %r14, 32(%rdi)\n"
    "    movq %r15, 40(%rdi)\n"
    "    leaq 8(%rsp), %rdx\n" /* the return address is popped by ret */
    "    movq %rdx, 48(%rdi)\n"
    "    movq (%rsp), %rdx\n"
    "    movq %rdx, 56(%rdi)\n"
    "    xorl %eax, %eax\n"
    "    ret\n"
    ".global longjmp\n"
    ".type longjmp, @function\n"
    "longjmp:\n"
    "    movl %esi, %eax\n"
    "    testl %eax, %eax\n"
    "    jnz 1f\n"
    "    incl %eax\n" /* longjmp(b, 0) makes setjmp return 1 */
    "1:  movq (%rdi), %rbx\n"
    "    movq 8(%rdi), %rbp\n"
    "    movq 16(%rdi), %r12\n"
    "    movq 24(%rdi), %r13\n"
    "    movq 32(%rdi), %r14\n"
    "    movq 40(%rdi), %r15\n"
    "    movq 48(%rdi), %rsp\n"
    "    jmp *56(%rdi)\n");

#elif defined(__aarch64__)

/* jmp_buf: x19 to x28, the frame pointer x29, the link register x30
 * (where setjmp returns to), the stack pointer, and d8 to d15 - the low
 * halves of v8 to v15, which this convention also counts as
 * callee-saved. */
__asm__(
    ".text\n"
    ".global setjmp\n"
    ".type setjmp, %function\n"
    "setjmp:\n"
    "    stp x19, x20, [x0]\n"
    "    stp x21, x22, [x0, #16]\n"
    "    stp x23, x24, [x0, #32]\n"
    "    stp x25, x26, [x0, #48]\n"
    "    stp x27, x28, [x0, #64]\n"
    "    stp x29, x30, [x0, #80]\n"
    "    mov x2, sp\n"
    "    str x2, [x0, #96]\n"
    "    stp d8, d9, [x0, #104]\n"
    "    stp d10, d11, [x0, #120]\n"
    "    stp d12, d13, [x0, #136]\n"
    "    stp d14, d15, [x0, #152]\n"
    "    mov w0, #0\n"
    "    ret\n"
    ".global longjmp\n"
    ".type longjmp, %function\n"
    "longjmp:\n"
    "    ldp x19, x20, [x0]\n"
    "    ldp x21, x22, [x0, #16]\n"
    "    ldp x23, x24, [x0, #32]\n"
    "    ldp x25, x26, [x0, #48]\n"
    "    ldp x27, x28, [x0, #64]\n"
    "    ldp x29, x30, [x0, #80]\n"
    "    ldr x2, [x0, #96]\n"
    "    mov sp, x2\n"
    "    ldp d8, d9, [x0, #104]\n"
    "    ldp d10, d11, [x0, #120]\n"
    "    ldp d12, d13, [x0, #136]\n"
    "    ldp d14, d15, [x0, #152]\n"
    /* value, or 1 if it is 0. A branch rather than cmp and csinc:
     * TCC's assembler for this CPU does not know those two. */
    "    cbnz w1, 1f\n"
    "    mov w1, #1\n"
    "1:  mov w0, w1\n"
    "    br x30\n");

#else
#error "setjmp and longjmp are written for x86_64 and AArch64 only"
#endif
