/* poll.h - waiting for any of several descriptors (libc/src/poll.rs says
 * what "ready" means for each kind). */
#ifndef _POLL_H
#define _POLL_H

#ifdef __cplusplus
extern "C" {
#endif

#define POLLIN     0x001 /* there is something to read */
#define POLLPRI    0x002 /* urgent data: never, here */
#define POLLOUT    0x004 /* a write would not wait */
#define POLLERR    0x008 /* an error: a pipe nobody reads any more */
#define POLLHUP    0x010 /* the other end closed: reading gives the end */
#define POLLNVAL   0x020 /* no such descriptor */
#define POLLRDNORM 0x040
#define POLLRDBAND 0x080
#define POLLWRNORM 0x100
#define POLLWRBAND 0x200

typedef unsigned long nfds_t;

struct pollfd {
    int fd;        /* negative: skipped */
    short events;  /* what to wait for */
    short revents; /* what it is ready for (POLLERR, POLLHUP, POLLNVAL
                      whether asked for or not) */
};

/* Waits until one of the n descriptors is ready, for at most timeout
 * milliseconds (negative: no limit; 0: just look). How many are ready;
 * 0 on the timeout; -1 with EINTR when a signal's handler ran. Time
 * goes in ticks of 10 ms. */
int poll(struct pollfd *fds, nfds_t n, int timeout);

#ifdef __cplusplus
}
#endif

#endif
