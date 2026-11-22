/* unistd.h - what the system itself does for a program.
 *
 * The functions are in libc.a (libc/src/lib.rs). A header only says
 * what they are called and what they take; the linker finds them. */
#ifndef _UNISTD_H
#define _UNISTD_H

#include <sys/types.h>

/* The descriptors every program starts with. */
#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

/* A descriptor is a small number standing for something open: one of
 * the three above, or a file that open (fcntl.h) returned. Each of
 * these returns -1 when it fails, with the reason in errno (errno.h). */

/* Writes count bytes to descriptor fd, where it has got to. Returns
 * how many. */
ssize_t write(int fd, const void *buffer, size_t count);

/* Reads up to count bytes. Returns how many: fewer than asked is not
 * the end; 0 is. */
ssize_t read(int fd, void *buffer, size_t count);

/* Done with the descriptor: its number may be given out again. */
int close(int fd);

/* Moves where the next read or write of a file starts: offset bytes
 * from the start, from where it is now, or from the end. Returns the
 * new place, counted from the start. */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
off_t lseek(int fd, off_t offset, int whence);

/* Removes a file. */
int unlink(const char *path);

/* 1 if the descriptor is the screen; 0 if it is a file or another
 * program (errno ENOTTY), or not open at all (EBADF). */
int isatty(int fd);

/* Ends the program at once. */
_Noreturn void _exit(int code);

/* The directory the program was started in, as a path from the root,
 * into buffer (size bytes) - or, for NULL, into memory from malloc.
 * NULL, with errno ERANGE, if it does not fit. */
char *getcwd(char *buffer, size_t size);

/* Replaces the program with another. TheTaaJ starts programs as new
 * processes and never replaces one: this always fails (ENOSYS). */
int execvp(const char *file, char *const argv[]);

#endif
