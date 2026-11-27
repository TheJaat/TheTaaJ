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

/* Interval timers (libc's timers.rs): ITIMER_REAL only, SIGALRM after
 * it_value and then every it_interval, exact to the kernel's one-shot
 * timer. The same alarm as alarm() and timer_create's: setting one
 * replaces the others. ITIMER_VIRTUAL and ITIMER_PROF are EINVAL. */
#define ITIMER_REAL    0
#define ITIMER_VIRTUAL 1
#define ITIMER_PROF    2
struct itimerval {
    struct timeval it_interval;
    struct timeval it_value;
};
int setitimer(int which, const struct itimerval *new_value, struct itimerval *old_value);
int getitimer(int which, struct itimerval *now);

/* Marks the file changed now, with times null; with times, fails with
 * EPERM (sys/stat.h says why). */
int utimes(const char *path, const struct timeval times[2]);

#ifdef __cplusplus
}
#endif

/* select, as Linux's sys/time.h brings it. */
#include <sys/select.h>

#endif
