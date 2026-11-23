/* spawn.h - starting programs: posix_spawn and posix_spawnp.
 *
 * A program on TheTaaJ is started from its file, as a new process,
 * through init - as the shell starts them (libc/src/spawn.rs). The
 * program that starts it must have been given the service "init" (the
 * shell's `with programs`); otherwise EPERM.
 *
 * The new program gets its arguments and name (argv), its file, this
 * program's current directory, standard output and error, and services.
 * No standard input; no environment (there is none). File actions and
 * attributes cannot be added yet: the add functions return ENOSYS.
 *
 * Wait for it with waitpid (sys/wait.h). */

#ifndef _SPAWN_H
#define _SPAWN_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    int __actions;
    int __reserved[7];
} posix_spawn_file_actions_t;

typedef struct {
    int __flags;
    int __reserved[7];
} posix_spawnattr_t;

int posix_spawn(pid_t *pid, const char *path, const posix_spawn_file_actions_t *actions,
                const posix_spawnattr_t *attributes, char *const argv[], char *const envp[]);
/* As posix_spawn, but a name with no '/' is a command: a file in the
 * root, as the shell runs it. */
int posix_spawnp(pid_t *pid, const char *file, const posix_spawn_file_actions_t *actions,
                 const posix_spawnattr_t *attributes, char *const argv[], char *const envp[]);

int posix_spawn_file_actions_init(posix_spawn_file_actions_t *actions);
int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *actions);
int posix_spawn_file_actions_addopen(posix_spawn_file_actions_t *actions, int descriptor, const char *path,
                                     int flags, mode_t mode);
int posix_spawn_file_actions_addclose(posix_spawn_file_actions_t *actions, int descriptor);
int posix_spawn_file_actions_adddup2(posix_spawn_file_actions_t *actions, int from, int to);

int posix_spawnattr_init(posix_spawnattr_t *attributes);
int posix_spawnattr_destroy(posix_spawnattr_t *attributes);

#ifdef __cplusplus
}
#endif

#endif
