/* signal.h - signals, as far as a program on TheTaaJ has them.
 *
 * On Unix a signal is the kernel interrupting a program: Ctrl+C is
 * SIGINT, a bad pointer SIGSEGV, a timer SIGALRM. TheTaaJ's kernel
 * sends programs no signals: Ctrl+C and a crash end the program outright
 * (the shell and the kernel say so).
 *
 * What is here is the part that is the program's own business, and it
 * works as POSIX says:
 *
 *   signal, sigaction   set what a signal does: a handler, SIG_DFL or
 *                       SIG_IGN
 *   raise, kill(self)   send one to this program: the handler runs, or
 *                       the default - end the program, with exit code
 *                       128 + the signal's number, as a shell shows a
 *                       program killed by it - or nothing, if ignored
 *   sigprocmask         block signals: one raised while blocked waits,
 *                       and is delivered when it is unblocked
 *   sigset_t and its functions
 *
 * kill of another process fails with EPERM, and alarm never rings: both
 * would need the kernel to send signals. The numbers are Linux's. */

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

/* What sigaction's handler with SA_SIGINFO is told. */
typedef struct {
    int si_signo;
    int si_errno;
    int si_code;
    pid_t si_pid;
    void *si_addr;
} siginfo_t;

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
int pthread_sigmask(int how, const sigset_t *set, sigset_t *old);

#ifdef __cplusplus
}
#endif

#endif
