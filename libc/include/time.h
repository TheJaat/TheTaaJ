/* time.h - the time, in seconds, and the calendar.
 *
 * TheTaaJ has no clock that knows the date yet, so time() counts the
 * seconds since the system started, not since 1 January 1970: the dates
 * made from it fall in January 1970. There are no time zones: localtime
 * is gmtime. (libc/src/system.rs, libc/c/time.c) */
#ifndef _TIME_H
#define _TIME_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stddef.h>

typedef long time_t;

struct tm {
    int tm_sec;   /* 0 to 59 */
    int tm_min;   /* 0 to 59 */
    int tm_hour;  /* 0 to 23 */
    int tm_mday;  /* 1 to 31 */
    int tm_mon;   /* 0 to 11 */
    int tm_year;  /* years since 1900 */
    int tm_wday;  /* 0 to 6, Sunday 0 */
    int tm_yday;  /* 0 to 365 */
    int tm_isdst; /* always 0 here */
};

/* The seconds, also stored in *t if t is not NULL. */
time_t time(time_t *t);

/* The calendar date and time of a count of seconds: into memory the
 * library keeps (the next call overwrites it), or the _r versions,
 * into out. */
struct tm *gmtime(const time_t *when);
struct tm *localtime(const time_t *when);
struct tm *gmtime_r(const time_t *when, struct tm *out);
struct tm *localtime_r(const time_t *when, struct tm *out);

/* The calendar back into seconds: the inverse of localtime (which is
 * gmtime here). The fields may be out of range - 32 January, -1
 * seconds - and are put right, in *t too. -1 if the time cannot be
 * held. (time.c) */
time_t mktime(struct tm *t);

/* end - start, in seconds. */
double difftime(time_t end, time_t start);

/* The time and date as text, by a format like printf's: %Y-%m-%d is
 * 2026-11-22. Writes at most `size` bytes with the zero; returns how
 * many it wrote without it, or 0 if they did not fit. All of C99's
 * conversions, with "C"'s English names. (time.c) */
size_t strftime(char *to, size_t size, const char *format, const struct tm *t);

/* "Sun Nov 22 17:40:00 2026\n": asctime of a calendar time, ctime of
 * seconds. The string is the library's, overwritten by the next call. */
char *asctime(const struct tm *t);
char *ctime(const time_t *when);

/* The processor time the program has used, all its threads together,
 * in units of CLOCKS_PER_SEC a second: CLOCK_PROCESS_CPUTIME_ID's time.
 * (system.rs) */
typedef long clock_t;
#define CLOCKS_PER_SEC 1000000L
clock_t clock(void);

/* Seconds and nanoseconds. */
struct timespec {
    time_t tv_sec;
    long tv_nsec;
};

/* clock_gettime(clock, t): the time on one of three clocks, to the
 * nanosecond the CPU's counter gives: CLOCK_REALTIME, since 1970 (UTC,
 * from the real-time clock at boot); CLOCK_MONOTONIC, since the system
 * started; or CLOCK_PROCESS_CPUTIME_ID, the CPU time this program has
 * used, all its threads together. None is ever set back. 0, or -1 with
 * errno EINVAL for another clock. (system.rs) */
typedef int clockid_t;
#define CLOCK_REALTIME  0
#define CLOCK_MONOTONIC 1
#define CLOCK_PROCESS_CPUTIME_ID 2
int clock_gettime(clockid_t clock, struct timespec *t);

/* timespec_get(t, TIME_UTC): C11's way to the same time as
 * CLOCK_REALTIME. Returns TIME_UTC, or 0 for another base. */
#define TIME_UTC 1
int timespec_get(struct timespec *t, int base);

/* Sleeps for at least *wanted: in whole ticks of the system's clock
 * (TICKS_PER_SECOND, 100 a second), rounded up. Nothing can wake the
 * sleeper early (there are no signals), so *left, if given, is set to
 * zero. */
int nanosleep(const struct timespec *wanted, struct timespec *left);

#ifdef __cplusplus
}
#endif

#endif
