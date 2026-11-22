/* sys/time.h - the time to a finer grain than seconds. */
#ifndef _SYS_TIME_H
#define _SYS_TIME_H

#ifdef __cplusplus
extern "C" {
#endif

#include <time.h>

struct timeval {
    long tv_sec;  /* seconds (since the system started: see time.h) */
    long tv_usec; /* and microseconds: here, in steps of a tick, 10 ms */
};

/* The time now into *tv. The time zone is ignored (POSIX says to pass
 * NULL). Returns 0. */
int gettimeofday(struct timeval *tv, void *tz);

#ifdef __cplusplus
}
#endif

#endif
