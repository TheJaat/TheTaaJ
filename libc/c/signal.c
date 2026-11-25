/* signal.c - signals: handlers, the mask, raise and kill, and the way
 * the kernel's signals come in (signals.rs has the system calls).
 *
 * The kernel delivers a signal only if this file told it the signal is
 * caught: then it starts a thread of the program at
 * __taaj_signal_entry (below) - wherever it was, at the end of a system
 * call or between two instructions - with the registers it had saved
 * on its stack. A signal not caught does what the kernel does by
 * default: nothing, for a few (SIGCHLD and its kin), or the end of the
 * program, which a shell then reports as ended by that signal.
 *
 * So what the kernel is told follows what the program asked:
 *   caught   signals with a handler - and signals blocked now, unless
 *            ignored: a blocked signal must wait (in `pending`) rather
 *            than end the program, so it comes here to be kept
 *   ignored  signals set to SIG_IGN
 * and it is told again whenever either changes (`tell_kernel`).
 *
 * The mask and the pending set are the process's, not each thread's:
 * pthread_sigmask is sigprocmask. Both are changed with atomic
 * operations, because a handler can run in the middle of anything -
 * including a change to them. */

#include <errno.h>
#include <setjmp.h>
#include <signal.h>
#include <stdatomic.h> /* TCC: __atomic_load_n, __atomic_store_n, __ATOMIC_SEQ_CST */
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

int __taaj_signal_setup(uint64_t entry, uint64_t caught, uint64_t ignored, uint64_t on_stack);
int __taaj_signal_self(int signal);
int __taaj_signal_child(int id, int signal);
void __taaj_signal_entry(void);

static struct sigaction actions[NSIG];
static sigset_t blocked;
static sigset_t pending;

static int valid(int signal)
{
    return signal > 0 && signal < NSIG;
}

static sigset_t bit(int signal)
{
    return 1UL << signal;
}

/* What a signal does when nothing was said: most end the program;
 * these few are ignored (or, for SIGSTOP and its kin, would stop it,
 * which a program here cannot be). The kernel has the same list. */
static int ignored_by_default(int signal)
{
    return signal == SIGCHLD || signal == SIGURG || signal == SIGWINCH || signal == SIGCONT
        || signal == SIGSTOP || signal == SIGTSTP || signal == SIGTTIN || signal == SIGTTOU;
}

/* Signals that mean a fault: the instruction that raised one will raise
 * it again if the handler returns to it. */
static int is_fault(int signal)
{
    return signal == SIGSEGV || signal == SIGBUS || signal == SIGILL || signal == SIGFPE;
}

static sigset_t load(sigset_t *set)
{
    return __atomic_load_n(set, __ATOMIC_SEQ_CST);
}

/* Tells the kernel which signals to deliver here and which to drop. */
static void tell_kernel(void)
{
    sigset_t caught = 0, ignored = 0, on_stack = 0;
    sigset_t now_blocked = load(&blocked);
    for (int signal = 1; signal < NSIG; signal++) {
        sighandler_t handler = actions[signal].sa_handler;
        if (handler == SIG_IGN)
            ignored |= bit(signal);
        else if (handler != SIG_DFL || (now_blocked & bit(signal)))
            caught |= bit(signal);
        /* On the thread's signal stack (sigaltstack), if it has one. */
        if (actions[signal].sa_flags & SA_ONSTACK)
            on_stack |= bit(signal);
    }
    __taaj_signal_setup((uint64_t)(uintptr_t)__taaj_signal_entry, caught, ignored, on_stack);
}

/* The program ends by `signal`, as if nothing had caught it: the kernel
 * is told it is not caught any more, and sent it. */
_Noreturn static void die(int signal)
{
    actions[signal].sa_handler = SIG_DFL;
    __atomic_fetch_and(&blocked, ~bit(signal), __ATOMIC_SEQ_CST);
    tell_kernel();
    __taaj_signal_self(signal);
    _exit(128 + signal); /* not reached: the kernel ends it on the way back */
}

static void deliver_pending(void);

/* Runs what `signal` does now that it is not blocked. */
static void deliver(int signal, void *context)
{
    struct sigaction action = actions[signal];
    if (action.sa_handler == SIG_IGN)
        return;
    if (action.sa_handler == SIG_DFL) {
        if (ignored_by_default(signal))
            return;
        die(signal);
    }
    if (action.sa_flags & SA_RESETHAND)
        actions[signal].sa_handler = SIG_DFL;
    /* While the handler runs, its mask is blocked too - and the signal
     * itself, unless SA_NODEFER. */
    sigset_t extra = action.sa_mask;
    if (!(action.sa_flags & SA_NODEFER))
        extra |= bit(signal);
    sigset_t before = __atomic_fetch_or(&blocked, extra, __ATOMIC_SEQ_CST);
    if (extra & ~before)
        tell_kernel();
    if (action.sa_flags & SA_SIGINFO) {
        siginfo_t info = {0};
        info.si_signo = signal;
        info.si_pid = getpid();
        action.sa_sigaction(signal, &info, context);
    } else {
        action.sa_handler(signal);
    }
    __atomic_store_n(&blocked, before, __ATOMIC_SEQ_CST);
    tell_kernel();
    deliver_pending();
}

/* Delivers what waited while it was blocked and no longer is. */
static void deliver_pending(void)
{
    for (int signal = 1; signal < NSIG; signal++) {
        if ((load(&pending) & bit(signal)) && !(load(&blocked) & bit(signal))) {
            sigset_t was = __atomic_fetch_and(&pending, ~bit(signal), __ATOMIC_SEQ_CST);
            if (was & bit(signal))
                deliver(signal, 0);
        }
    }
}

/* Where the entry below brings every signal the kernel delivers:
 * `context` is the abi::signal::Context of the code it interrupted. */
void __taaj_signal_dispatch(int signal, void *context)
{
    if (!valid(signal))
        return;
    int is_blocked = (load(&blocked) & bit(signal)) != 0;
    /* A fault that cannot be handled - blocked, or meant to end the
     * program - must not return to its instruction, which would only
     * fault again: it ends the program now. */
    if (is_fault(signal) && (is_blocked || actions[signal].sa_handler == SIG_DFL))
        die(signal);
    if (is_blocked) {
        __atomic_fetch_or(&pending, bit(signal), __ATOMIC_SEQ_CST);
        return;
    }
    deliver(signal, context);
}

/* The entry: started by the kernel with the signal in the first
 * argument register and the context in the second, the stack pointer
 * at the context (16-byte aligned). The kernel saved the general
 * registers; the floating-point ones are saved here, around the C
 * code, which may use them. Then signal_return (37) with the context:
 * the kernel puts everything back as it was, and never returns here.
 *
 * The instructions TCC's assemblers do not know - fxsave64 and
 * fxrstor64; AArch64's q registers, FPSR/FPCR, svc and brk - are
 * written as their encodings (what clang assembles them to), each
 * named beside it: TheTaaJ compiles this file with TCC too. */
#if defined(__x86_64__)
__asm__(".text\n"
        ".global __taaj_signal_entry\n"
        ".type __taaj_signal_entry, @function\n"
        "__taaj_signal_entry:\n"
        "    cld\n"                     /* C code expects the direction flag clear */
        "    mov %rsi, %rbx\n"          /* the context, across the call (rbx is saved in it) */
        "    sub $512, %rsp\n"          /* fxsave's 512 bytes, still 16-byte aligned */
        "    .byte 0x48, 0x0f, 0xae, 0x04, 0x24\n" /* fxsave64 (%rsp) */
        "    call __taaj_signal_dispatch\n"
        "    .byte 0x48, 0x0f, 0xae, 0x0c, 0x24\n" /* fxrstor64 (%rsp) */
        "    mov %rbx, %rdi\n"
        "    mov $37, %eax\n"
        "    syscall\n"
        "    ud2\n");
#elif defined(__aarch64__)
__asm__(".text\n"
        ".global __taaj_signal_entry\n"
        ".type __taaj_signal_entry, %function\n"
        "__taaj_signal_entry:\n"
        "    mov x19, x1\n"             /* the context, across the call (x19 is saved in it) */
        "    sub sp, sp, #528\n"        /* FPSR and FPCR, then q0-q31 */
        "    .int 0xad0087e0\n" /* stp q0, q1, [sp, #16] */
        "    .int 0xad018fe2\n" /* stp q2, q3, [sp, #48] */
        "    .int 0xad0297e4\n" /* stp q4, q5, [sp, #80] */
        "    .int 0xad039fe6\n" /* stp q6, q7, [sp, #112] */
        "    .int 0xad04a7e8\n" /* stp q8, q9, [sp, #144] */
        "    .int 0xad05afea\n" /* stp q10, q11, [sp, #176] */
        "    .int 0xad06b7ec\n" /* stp q12, q13, [sp, #208] */
        "    .int 0xad07bfee\n" /* stp q14, q15, [sp, #240] */
        "    .int 0xad08c7f0\n" /* stp q16, q17, [sp, #272] */
        "    .int 0xad09cff2\n" /* stp q18, q19, [sp, #304] */
        "    .int 0xad0ad7f4\n" /* stp q20, q21, [sp, #336] */
        "    .int 0xad0bdff6\n" /* stp q22, q23, [sp, #368] */
        "    .int 0xad0ce7f8\n" /* stp q24, q25, [sp, #400] */
        "    .int 0xad0deffa\n" /* stp q26, q27, [sp, #432] */
        "    .int 0xad0ef7fc\n" /* stp q28, q29, [sp, #464] */
        "    .int 0xad0ffffe\n" /* stp q30, q31, [sp, #496] */
        "    .int 0xd53b4429\n" /* mrs x9, fpsr */
        "    .int 0xd53b440a\n" /* mrs x10, fpcr */
        "    stp x9, x10, [sp, #0]\n"
        "    bl __taaj_signal_dispatch\n"
        "    ldp x9, x10, [sp, #0]\n"
        "    .int 0xd51b4429\n" /* msr fpsr, x9 */
        "    .int 0xd51b440a\n" /* msr fpcr, x10 */
        "    .int 0xad4087e0\n" /* ldp q0, q1, [sp, #16] */
        "    .int 0xad418fe2\n" /* ldp q2, q3, [sp, #48] */
        "    .int 0xad4297e4\n" /* ldp q4, q5, [sp, #80] */
        "    .int 0xad439fe6\n" /* ldp q6, q7, [sp, #112] */
        "    .int 0xad44a7e8\n" /* ldp q8, q9, [sp, #144] */
        "    .int 0xad45afea\n" /* ldp q10, q11, [sp, #176] */
        "    .int 0xad46b7ec\n" /* ldp q12, q13, [sp, #208] */
        "    .int 0xad47bfee\n" /* ldp q14, q15, [sp, #240] */
        "    .int 0xad48c7f0\n" /* ldp q16, q17, [sp, #272] */
        "    .int 0xad49cff2\n" /* ldp q18, q19, [sp, #304] */
        "    .int 0xad4ad7f4\n" /* ldp q20, q21, [sp, #336] */
        "    .int 0xad4bdff6\n" /* ldp q22, q23, [sp, #368] */
        "    .int 0xad4ce7f8\n" /* ldp q24, q25, [sp, #400] */
        "    .int 0xad4deffa\n" /* ldp q26, q27, [sp, #432] */
        "    .int 0xad4ef7fc\n" /* ldp q28, q29, [sp, #464] */
        "    .int 0xad4ffffe\n" /* ldp q30, q31, [sp, #496] */
        "    mov x0, x19\n"
        "    mov x8, #37\n"
        "    .int 0xd4000001\n" /* svc #0 */
        "    .int 0xd4200000\n" /* brk #0 */);
#else
#error "signal.c: no signal entry for this CPU"
#endif

int sigaction(int signal, const struct sigaction *action, struct sigaction *old)
{
    if (!valid(signal)) {
        errno = EINVAL;
        return -1;
    }
    if (old)
        *old = actions[signal];
    if (action) {
        if (signal == SIGKILL || signal == SIGSTOP) {
            errno = EINVAL; /* POSIX: these two cannot be caught or ignored */
            return -1;
        }
        actions[signal] = *action;
        tell_kernel();
    }
    return 0;
}

sighandler_t signal(int signal, sighandler_t handler)
{
    struct sigaction action = {0}, old;
    action.sa_handler = handler;
    if (sigaction(signal, &action, &old) != 0)
        return SIG_ERR;
    return old.sa_handler;
}

/* Through the kernel, like any other signal: caught, it is delivered -
 * its handler run - before the call returns; blocked, it waits; not
 * caught, it ends the program. */
int raise(int signal)
{
    if (!valid(signal)) {
        errno = EINVAL;
        return -1;
    }
    return __taaj_signal_self(signal);
}

/* This program, or a child it started (signals.rs says why no other).
 * Signal 0 asks only "is it there?". */
int kill(pid_t process, int signal)
{
    if (signal < 0 || signal >= NSIG) {
        errno = EINVAL;
        return -1;
    }
    if (process == 0 || process == getpid())
        return signal == 0 ? 0 : raise(signal);
    int error = __taaj_signal_child(process, signal);
    if (error) {
        errno = error;
        return -1;
    }
    return 0;
}

int sigemptyset(sigset_t *set)
{
    *set = 0;
    return 0;
}

int sigfillset(sigset_t *set)
{
    *set = ~0UL;
    return 0;
}

int sigaddset(sigset_t *set, int signal)
{
    if (!valid(signal)) {
        errno = EINVAL;
        return -1;
    }
    *set |= bit(signal);
    return 0;
}

int sigdelset(sigset_t *set, int signal)
{
    if (!valid(signal)) {
        errno = EINVAL;
        return -1;
    }
    *set &= ~bit(signal);
    return 0;
}

int sigismember(const sigset_t *set, int signal)
{
    if (!valid(signal)) {
        errno = EINVAL;
        return -1;
    }
    return (*set & bit(signal)) != 0;
}

int sigpending(sigset_t *set)
{
    *set = load(&pending);
    return 0;
}

int sigprocmask(int how, const sigset_t *set, sigset_t *old)
{
    sigset_t was = load(&blocked);
    if (set) {
        sigset_t now;
        switch (how) {
        case SIG_BLOCK: now = was | *set; break;
        case SIG_UNBLOCK: now = was & ~*set; break;
        case SIG_SETMASK: now = *set; break;
        default: errno = EINVAL; return -1;
        }
        now &= ~(bit(SIGKILL) | bit(SIGSTOP)); /* these two cannot be blocked */
        __atomic_store_n(&blocked, now, __ATOMIC_SEQ_CST);
        tell_kernel();
        deliver_pending();
    }
    if (old)
        *old = was;
    return 0;
}

/* With the mask the process's, a thread's mask is the process's. */
int pthread_sigmask(int how, const sigset_t *set, sigset_t *old)
{
    return sigprocmask(how, set, old) == 0 ? 0 : errno;
}

/* Waits, with `mask` as the mask, until a handler has run; the mask is
 * then as it was. Always -1 with EINTR. One that was pending and is
 * unblocked by `mask` runs at once. */
int sigsuspend(const sigset_t *mask)
{
    sigset_t was;
    sigprocmask(SIG_SETMASK, mask, &was);
    int ran = pause();
    sigprocmask(SIG_SETMASK, &was, 0);
    (void)ran;
    errno = EINTR;
    return -1;
}

/* strsignal: each signal's name, in words. */
char *strsignal(int signal)
{
    static const char *const names[NSIG] = {
        [SIGHUP] = "hang up", [SIGINT] = "interrupted", [SIGQUIT] = "quit",
        [SIGILL] = "illegal instruction", [SIGTRAP] = "trap", [SIGABRT] = "aborted",
        [SIGBUS] = "bus error", [SIGFPE] = "arithmetic exception", [SIGKILL] = "killed",
        [SIGUSR1] = "user signal 1", [SIGSEGV] = "bad memory access", [SIGUSR2] = "user signal 2",
        [SIGPIPE] = "broken pipe", [SIGALRM] = "alarm clock", [SIGTERM] = "terminated",
        [SIGSTKFLT] = "stack fault", [SIGCHLD] = "child ended", [SIGCONT] = "continued",
        [SIGSTOP] = "stopped (signal)", [SIGTSTP] = "stopped", [SIGTTIN] = "stopped (terminal input)",
        [SIGTTOU] = "stopped (terminal output)", [SIGURG] = "urgent data", [SIGXCPU] = "processor time used up",
        [SIGXFSZ] = "file too big", [SIGVTALRM] = "virtual alarm clock", [SIGPROF] = "profiling alarm clock",
        [SIGWINCH] = "window changed", [SIGIO] = "input or output possible", [SIGPWR] = "power failure",
        [SIGSYS] = "bad system call",
    };
    if (signal > 0 && signal < NSIG && names[signal])
        return (char *)names[signal];
    return (char *)"unknown signal";
}

/* sigsetjmp's other half (setjmp.h): the blocked signals, kept... */
void __taaj_sigjmp_save(unsigned long *mask)
{
    sigprocmask(SIG_SETMASK, NULL, mask);
}

/* ...and put back, before the jump. */
_Noreturn void siglongjmp(sigjmp_buf buffer, int value)
{
    if (buffer->saved)
        sigprocmask(SIG_SETMASK, &buffer->mask, NULL);
    longjmp(buffer->jump, value);
}
