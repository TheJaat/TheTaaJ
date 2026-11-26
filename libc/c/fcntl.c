/* fcntl.c - open and fcntl, which are here and not with read and write
 * because of their three dots.
 *
 * open(path, flags) and open(path, flags, mode) are both right: the
 * third argument is given only when a file may be made. Taking "two
 * arguments or three" is something a C function can say and a Rust one
 * cannot, so these are C functions, and they call the Rust ones with
 * what matters here (libc/src/descriptors.rs). */

#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <sys/file.h>

int __taaj_open(const char *path, int flags);
int __taaj_openat(int dirfd, const char *path, int flags);
int __taaj_fcntl(int fd, int command, long argument);

int open(const char *path, int flags, ...)
{
    return __taaj_open(path, flags);
}

int openat(int dirfd, const char *path, int flags, ...)
{
    return __taaj_openat(dirfd, path, flags);
}

int fcntl(int fd, int command, ...)
{
    long argument = 0;
    va_list arguments;
    va_start(arguments, command);
    /* Every command here that takes a third argument takes an int, a
     * long or a pointer; a long holds any of them. */
    if (command != F_GETFD && command != F_GETFL)
        argument = va_arg(arguments, long);
    va_end(arguments);
    return __taaj_fcntl(fd, command, argument);
}

/* BSD's whole-file locks: none are kept here (sys/file.h). */
int flock(int fd, int operation)
{
    (void)fd;
    (void)operation;
    errno = ENOLCK;
    return -1;
}
