/* time.h - the time, in seconds, and the calendar.
 *
 * TheTaaJ has no clock that knows the date yet, so time() counts the
 * seconds since the system started, not since 1 January 1970: the dates
 * made from it fall in January 1970. There are no time zones: localtime
 * is gmtime. (libc/src/system.rs, libc/c/time.c) */
#ifndef _TIME_H
#define _TIME_H

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

#endif
