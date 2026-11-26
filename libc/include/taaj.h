/* taaj.h - what TheTaaJ's C library has that POSIX does not. */
#ifndef _TAAJ_H
#define _TAAJ_H

#ifdef __cplusplus
extern "C" {
#endif

/* A namespace per process (journal entry 117, J62). Every name this
 * program uses is worked out by its C library, not by the kernel: a
 * directory mounted at a path is seen there, in place of what was
 * there, by this program and every program it starts from now on.
 * The directory is a channel to its file service, so the mount is
 * only this program's: no other program sees it, and nothing has to
 * be asked of anyone to make it. */

/* taaj_mount's flag: changes under the mount are refused (EROFS). */
#define TAAJ_MOUNT_READ_ONLY 1

/* The directory `directory` names is seen at `at` from now on. 0, or
 * -1 and errno: ENOENT, ENOTDIR, EROFS, EINVAL (an unknown flag). */
int taaj_mount(const char *at, const char *directory, int flags);

/* What is mounted at `at` is not any more. 0, or -1 and errno EINVAL
 * if nothing is mounted there. */
int taaj_unmount(const char *at);

#ifdef __cplusplus
}
#endif

#endif
