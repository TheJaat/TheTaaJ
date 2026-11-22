/* signal.c - a program's own signals: handlers, raise, the mask.
 * signal.h says what is here and what is not (the kernel sends none). */

#include <errno.h>
#include <signal.h>
#include <stdlib.h>
#include <unistd.h>

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
 * which a program here cannot be). */
static int ignored_by_default(int signal)
{
    return signal == SIGCHLD || signal == SIGURG || signal == SIGWINCH || signal == SIGCONT
        || signal == SIGSTOP || signal == SIGTSTP || signal == SIGTTIN || signal == SIGTTOU;
}

static void deliver(int signal)
{
    struct sigaction action = actions[signal];
    if (action.sa_handler == SIG_IGN)
        return;
    if (action.sa_handler == SIG_DFL) {
        if (ignored_by_default(signal))
            return;
        /* A shell shows a program a signal ended as 128 + its number. */
        exit(128 + signal);
    }
    if (action.sa_flags & SA_RESETHAND)
        actions[signal].sa_handler = SIG_DFL;
    /* While the handler runs, its mask is blocked too - and the signal
     * itself, unless SA_NODEFER. */
    sigset_t before = blocked;
    blocked |= action.sa_mask;
    if (!(action.sa_flags & SA_NODEFER))
        blocked |= bit(signal);
    if (action.sa_flags & SA_SIGINFO) {
        siginfo_t info = {signal, 0, 0, getpid(), 0};
        action.sa_sigaction(signal, &info, 0);
    } else {
        action.sa_handler(signal);
    }
    blocked = before;
}

/* Delivers what waited while it was blocked and no longer is. */
static void deliver_pending(void)
{
    for (int signal = 1; signal < NSIG; signal++) {
        if ((pending & bit(signal)) && !(blocked & bit(signal))) {
            pending &= ~bit(signal);
            deliver(signal);
        }
    }
}

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

int raise(int signal)
{
    if (!valid(signal)) {
        errno = EINVAL;
        return -1;
    }
    if (signal == SIGKILL)
        exit(128 + SIGKILL); /* never blocked, caught or ignored */
    if (blocked & bit(signal)) {
        pending |= bit(signal);
        return 0;
    }
    deliver(signal);
    return 0;
}

/* Only this program can be reached: another would need the kernel to
 * send it. Signal 0 asks only "is it there?". */
int kill(pid_t process, int signal)
{
    if (signal < 0 || signal >= NSIG) {
        errno = EINVAL;
        return -1;
    }
    if (process != 0 && process != getpid()) {
        errno = EPERM;
        return -1;
    }
    return signal == 0 ? 0 : raise(signal);
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

int sigprocmask(int how, const sigset_t *set, sigset_t *old)
{
    if (old)
        *old = blocked;
    if (set) {
        switch (how) {
        case SIG_BLOCK: blocked |= *set; break;
        case SIG_UNBLOCK: blocked &= ~*set; break;
        case SIG_SETMASK: blocked = *set; break;
        default: errno = EINVAL; return -1;
        }
        blocked &= ~(bit(SIGKILL) | bit(SIGSTOP)); /* these two cannot be blocked */
        deliver_pending();
    }
    return 0;
}

/* With one thread, the thread's mask is the process's. */
int pthread_sigmask(int how, const sigset_t *set, sigset_t *old)
{
    return sigprocmask(how, set, old) == 0 ? 0 : errno;
}

/* An alarm is SIGALRM sent by the kernel when the time is up, and the
 * kernel sends no signals: so it never rings, and there is never an
 * earlier alarm whose seconds are left (0). */
unsigned alarm(unsigned seconds)
{
    (void)seconds;
    return 0;
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
