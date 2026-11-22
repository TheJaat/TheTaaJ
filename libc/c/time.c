/* time.c - calendar time from seconds: gmtime and localtime.
 *
 * time() itself is in the Rust half (libc/src/system.rs): TheTaaJ has
 * no clock that knows the date yet, so its seconds count from the
 * moment the system started, and the dates made here from them fall in
 * January 1970. There are no time zones either: localtime is gmtime. */

#include <time.h>

/* The civil date of day `days` after 1 January 1970 - the proleptic
 * Gregorian calendar, by Howard Hinnant's method: count in "eras" of
 * 400 years (each exactly 146097 days), and start each year on 1 March,
 * so that February, with its leap day, comes last. */
static void civil(long days, int *year, int *month, int *day)
{
    days += 719468; /* from 1 March of year 0 */
    long era = (days >= 0 ? days : days - 146096) / 146097;
    long of_era = days - era * 146097;                                         /* 0 to 146096 */
    long year_of_era = (of_era - of_era / 1460 + of_era / 36524 - of_era / 146096) / 365; /* 0 to 399 */
    long day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); /* 0 to 365 */
    long shifted_month = (5 * day_of_year + 2) / 153;                          /* 0 = March */
    *day = (int)(day_of_year - (153 * shifted_month + 2) / 5 + 1);
    *month = (int)(shifted_month < 10 ? shifted_month + 3 : shifted_month - 9); /* 1 to 12 */
    *year = (int)(year_of_era + era * 400 + (*month <= 2));
}

static int leap(int year)
{
    return (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
}

struct tm *gmtime_r(const time_t *when, struct tm *out)
{
    long seconds = *when;
    long days = seconds / 86400;
    long rest = seconds % 86400;
    if (rest < 0) {
        rest += 86400;
        days--;
    }
    int year, month, day;
    civil(days, &year, &month, &day);
    static const short before[12] = {0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334};
    out->tm_sec = (int)(rest % 60);
    out->tm_min = (int)(rest / 60 % 60);
    out->tm_hour = (int)(rest / 3600);
    out->tm_mday = day;
    out->tm_mon = month - 1;
    out->tm_year = year - 1900;
    /* 1 January 1970 was a Thursday (4). */
    out->tm_wday = (int)(((days % 7) + 11) % 7);
    out->tm_yday = before[month - 1] + day - 1 + (month > 2 && leap(year));
    out->tm_isdst = 0;
    return out;
}

struct tm *gmtime(const time_t *when)
{
    static struct tm kept;
    return gmtime_r(when, &kept);
}

struct tm *localtime(const time_t *when)
{
    return gmtime(when);
}

struct tm *localtime_r(const time_t *when, struct tm *out)
{
    return gmtime_r(when, out);
}
