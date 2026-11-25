/* signal.h - signals: the kernel interrupting a program, as on Unix.
 * Ctrl+C is SIGINT, a bad pointer SIGSEGV, an alarm SIGALRM.
 *
 *   signal, sigaction   set what a signal does: a handler, SIG_DFL or
 *                       SIG_IGN. A handler runs on the thread the kernel
 *                       interrupted, on its own stack, and the program
 *                       goes on where it was when it returns
 *   raise, kill         send one: to this program, or to a child it
 *                       started (another is EPERM: TheTaaJ gives a
 *                       program no way to reach a process it was not
 *                       handed). One not caught does what it does by
 *                       default - ends the program, which a shell then
 *                       reports as ended by that signal; or nothing,
 *                       for SIGCHLD and its kin
 *   sigprocmask         block signals: one that comes while blocked
 *                       waits, and is delivered when it is unblocked
 *   sigpending, sigsuspend, and sigset_t and its functions
 *   alarm, pause        (unistd.h)
 *
 * A call that waits - for input, a child, a lock - and is interrupted
 * by a handler goes on waiting afterwards (POSIX's SA_RESTART, always);
 * only nanosleep, pause and sigsuspend return EINTR. The mask is the
 * process's, not each thread's. The numbers are Linux's. */

#ifndef _SIGNAL_H
#define _SIGNAL_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define SIGHUP 1
#define SIGINT 2
#define SIGQUIT 3
#define SIGILL 4
#define SIGTRAP 5
#define SIGABRT 6
#define SIGIOT SIGABRT
#define SIGBUS 7
#define SIGFPE 8
#define SIGKILL 9
#define SIGUSR1 10
#define SIGSEGV 11
#define SIGUSR2 12
#define SIGPIPE 13
#define SIGALRM 14
#define SIGTERM 15
#define SIGSTKFLT 16
#define SIGCHLD 17
#define SIGCONT 18
#define SIGSTOP 19
#define SIGTSTP 20
#define SIGTTIN 21
#define SIGTTOU 22
#define SIGURG 23
#define SIGXCPU 24
#define SIGXFSZ 25
#define SIGVTALRM 26
#define SIGPROF 27
#define SIGWINCH 28
#define SIGIO 29
#define SIGPWR 30
#define SIGSYS 31
#define NSIG 32
#define _NSIG NSIG

typedef int sig_atomic_t;
typedef void (*sighandler_t)(int);

#define SIG_DFL ((sighandler_t)0)
#define SIG_IGN ((sighandler_t)1)
#define SIG_ERR ((sighandler_t)-1)

/* A set of signals: bit n for signal n. */
typedef unsigned long sigset_t;

/* What sigaction's handler with SA_SIGINFO is told, and what waitid
 * fills in: for a child, its number (si_pid), how it ended (si_code: a
 * CLD_ value) and its exit code or signal (si_status). */
typedef struct {
    int si_signo;
    int si_errno;
    int si_code;
    pid_t si_pid;
    void *si_addr;
    uid_t si_uid;
    int si_status;
} siginfo_t;

/* si_code for SIGCHLD and waitid: the child exited, or a signal ended
 * it (CLD_DUMPED is never said: nothing is dumped). */
#define CLD_EXITED 1
#define CLD_KILLED 2
#define CLD_DUMPED 3

/* A signal stack (sigaltstack): where handlers installed with
 * SA_ONSTACK run - the only place one can when the thread's own stack
 * has overflowed. Each thread has its own. */
typedef struct {
    void *ss_sp;
    int ss_flags;
    size_t ss_size;
} stack_t;

#define SS_ONSTACK 1
#define SS_DISABLE 2
#define MINSIGSTKSZ 4096
#define SIGSTKSZ 16384

int sigaltstack(const stack_t *stack, stack_t *old);

struct sigaction {
    union {
        sighandler_t sa_handler;
        void (*sa_sigaction)(int, siginfo_t *, void *);
    };
    sigset_t sa_mask;  /* blocked while the handler runs */
    int sa_flags;
    void (*sa_restorer)(void);
};

#define SA_NOCLDSTOP 1
#define SA_NOCLDWAIT 2
#define SA_SIGINFO 4
#define SA_ONSTACK 0x08000000
#define SA_RESTART 0x10000000
#define SA_NODEFER 0x40000000
#define SA_RESETHAND 0x80000000

#define SIG_BLOCK 0
#define SIG_UNBLOCK 1
#define SIG_SETMASK 2

sighandler_t signal(int signal, sighandler_t handler);
int sigaction(int signal, const struct sigaction *action, struct sigaction *old);
int raise(int signal);
int kill(pid_t process, int signal);

int sigemptyset(sigset_t *set);
int sigfillset(sigset_t *set);
int sigaddset(sigset_t *set, int signal);
int sigdelset(sigset_t *set, int signal);
int sigismember(const sigset_t *set, int signal);
int sigprocmask(int how, const sigset_t *set, sigset_t *old);
int sigpending(sigset_t *set);
int sigsuspend(const sigset_t *mask);
int pthread_sigmask(int how, const sigset_t *set, sigset_t *old);

#ifdef __cplusplus
}
#endif

#endif
