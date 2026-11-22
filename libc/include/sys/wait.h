/* sys/wait.h - waiting for a child process to end.
 *
 * A C program on TheTaaJ cannot start another program yet (fork and
 * exec are not here, and posix_spawn not yet), so it never has a child:
 * waitpid and wait fail with ECHILD, as POSIX says they do then. */

#ifndef _SYS_WAIT_H
#define _SYS_WAIT_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define WNOHANG 1
#define WUNTRACED 2

/* What a status says: Linux's encoding. */
#define WEXITSTATUS(s) (((s) & 0xff00) >> 8)
#define WTERMSIG(s) ((s) & 0x7f)
#define WSTOPSIG(s) WEXITSTATUS(s)
#define WIFEXITED(s) (WTERMSIG(s) == 0)
#define WIFSIGNALED(s) (((signed char)(((s) & 0x7f) + 1) >> 1) > 0)
#define WIFSTOPPED(s) (((s) & 0xff) == 0x7f)

pid_t waitpid(pid_t process, int *status, int options);
pid_t wait(int *status);

struct rusage;
pid_t wait4(pid_t process, int *status, int options, struct rusage *usage);

#ifdef __cplusplus
}
#endif

#endif
