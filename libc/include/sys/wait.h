/* sys/wait.h - waiting for a child process to end.
 *
 * A child is a program this one started (posix_spawn, system, popen):
 * there is no fork. When one ends, this program is sent SIGCHLD, which
 * does nothing unless caught. */

#ifndef _SYS_WAIT_H
#define _SYS_WAIT_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define WNOHANG 1
#define WUNTRACED 2
/* waitid's options: which endings to report (only WEXITED happens
 * here: a process cannot be stopped), and WNOWAIT - report it, but
 * leave the child to be waited for again. */
#define WSTOPPED 2
#define WEXITED 4
#define WCONTINUED 8
#define WNOWAIT 0x01000000

/* What a status says: Linux's encoding. */
#define WEXITSTATUS(s) (((s) & 0xff00) >> 8)
#define WTERMSIG(s) ((s) & 0x7f)
#define WSTOPSIG(s) WEXITSTATUS(s)
#define WIFEXITED(s) (WTERMSIG(s) == 0)
#define WIFSIGNALED(s) (((signed char)(((s) & 0x7f) + 1) >> 1) > 0)
#define WIFSTOPPED(s) (((s) & 0xff) == 0x7f)

pid_t waitpid(pid_t process, int *status, int options);
pid_t wait(int *status);

/* waitid: which children - any, or one by number (there are no
 * process groups: P_PGID is refused, EINVAL). */
typedef enum { P_ALL = 0, P_PID = 1, P_PGID = 2 } idtype_t;
typedef unsigned int id_t;

#include <signal.h>
int waitid(idtype_t type, id_t id, siginfo_t *info, int options);

struct rusage;
pid_t wait4(pid_t process, int *status, int options, struct rusage *usage);

#ifdef __cplusplus
}
#endif

#endif
