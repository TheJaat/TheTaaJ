/* fcntl.c - open, which is here and not with read and write because of
 * its three dots.
 *
 * open(path, flags) and open(path, flags, mode) are both right: the
 * third argument is given only when a file may be made. Taking "two
 * arguments or three" is something a C function can say and a Rust one
 * cannot, so this is the C function, and it calls the Rust one with
 * the two that matter here (libc/src/descriptors.rs). */

#include <fcntl.h>

int __taaj_open(const char *path, int flags);

int open(const char *path, int flags, ...)
{
    return __taaj_open(path, flags);
}
