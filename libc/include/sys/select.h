/* sys/select.h - select, poll's older shape (libc/src/poll.rs). */
#ifndef _SYS_SELECT_H
#define _SYS_SELECT_H

#include <sys/time.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FD_SETSIZE 1024
#define __FD_BITS (8 * (int)sizeof(unsigned long))

typedef struct {
    unsigned long fds_bits[FD_SETSIZE / (8 * sizeof(unsigned long))];
} fd_set;

#define FD_ZERO(set) \
    do { for (unsigned __i = 0; __i < sizeof((set)->fds_bits) / sizeof(unsigned long); __i++) (set)->fds_bits[__i] = 0; } while (0)
#define FD_SET(fd, set)   ((set)->fds_bits[(fd) / __FD_BITS] |= 1UL << ((fd) % __FD_BITS))
#define FD_CLR(fd, set)   ((set)->fds_bits[(fd) / __FD_BITS] &= ~(1UL << ((fd) % __FD_BITS)))
#define FD_ISSET(fd, set) (((set)->fds_bits[(fd) / __FD_BITS] >> ((fd) % __FD_BITS)) & 1)

/* Waits until a descriptor below n set in read is readable, or one in
 * write writable; leaves only those set. except is emptied (no
 * out-of-band data here). timeout null: no limit. How many bits are
 * set; 0 on the timeout; -1 with EBADF, EINTR or EINVAL. */
int select(int n, fd_set *read, fd_set *write, fd_set *except, struct timeval *timeout);

#ifdef __cplusplus
}
#endif

#endif
