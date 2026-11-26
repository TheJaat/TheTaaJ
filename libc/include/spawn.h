/* spawn.h - starting programs: posix_spawn and posix_spawnp.
 *
 * A program on TheTaaJ is started from its file, as a new process,
 * through init - as the shell starts them (libc/src/spawn.rs). The
 * program that starts it must have been given the service "init" (the
 * shell's `with programs`); otherwise EPERM.
 *
 * The new program gets its arguments and name (argv), its file, this
 * program's current directory, its services and its descriptors, as
 * Unix's exec leaves them: every one not marked close-on-exec
 * (O_CLOEXEC, FD_CLOEXEC, pipe2) - but no standard input, unless a
 * file action gives it one. A file at 3 or above is handed down by its
 * name, where this program had got to and how it was opened; at 0, 1
 * or 2 it becomes a stream the file service keeps, read from where this
 * program had got to or added to the end. A pipe's end is handed down
 * as itself: the reader sees the end only once every program that was
 * given the write end has closed it - so mark a pipe meant for one
 * program close-on-exec (pipe2(fds, O_CLOEXEC)) and dup2 the end it is
 * to have.
 *
 * File actions are done, in order, on what would be the new program's
 * descriptors - this program's, but without standard input - and it
 * gets what comes out:
 *   adddup2(a, fd, to)    `to` is what `fd` is: this program's
 *                         standard ones, a pipe's end (unistd.h), a
 *                         file open here, or what an earlier action made
 *   addopen(a, fd, path, flags, mode)
 *                         `fd` open on `path`: for 0, 1 and 2 a stream
 *                         the file service keeps - read from the start
 *                         with O_RDONLY (and O_RDWR, for 0), else
 *                         written, from empty with O_TRUNC, at the end
 *                         otherwise; above, the file itself. ENOENT
 *                         without O_CREAT for a file not there. `mode`
 *                         means nothing (no permissions).
 *   addclose(a, fd)       the new program does not get `fd`
 * Attributes cannot be set: posix_spawn fails with ENOSYS if any are.
 * envp is the new program's environment (its environ); NULL passes this
 * program's own. It travels in init's request, so it is small: the
 * arguments and it, with the names, in 1024 bytes (else E2BIG). 
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
    int __pad;
    void *__list; /* the actions, kept by the library: destroy frees it */
    int __reserved[4];
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
/* TheTaaJ's own (journal entry NNN): the new program's whole tree is
 * the directory path names - "/" is that directory - and it is given
 * no other way to the files: a sandbox made of what it is handed. */
int posix_spawn_file_actions_addroot_np(posix_spawn_file_actions_t *actions, const char *path);

int posix_spawnattr_init(posix_spawnattr_t *attributes);
int posix_spawnattr_destroy(posix_spawnattr_t *attributes);

#ifdef __cplusplus
}
#endif

#endif
