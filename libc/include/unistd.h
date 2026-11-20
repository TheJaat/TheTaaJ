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

/* Writes count bytes to descriptor fd. Returns how many, or -1. */
ssize_t write(int fd, const void *buffer, size_t count);

/* Ends the program at once. */
_Noreturn void _exit(int code);

#endif
