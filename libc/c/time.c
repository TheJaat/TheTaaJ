/* time.c - calendar time from seconds and back: gmtime, localtime,
 * mktime, difftime; and as text: strftime, asctime, ctime.
 *
 * time() itself is in the Rust half (libc/src/system.rs): seconds since
 * 1970 from the kernel's clock, which the machine's real-time clock set
 * at boot. There are no time zones: localtime is gmtime, UTC. */

#include <stdio.h>
#include <string.h>
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

/* ---- back from the calendar ---- */

/* Days from 1 January 1970 to the given date: civil() undone. */
static long days_from_civil(long year, int month, int day)
{
    year -= month <= 2;
    long era = (year >= 0 ? year : year - 399) / 400;
    long year_of_era = year - era * 400;                                           /* 0 to 399 */
    long day_of_year = (153 * (month > 2 ? month - 3 : month + 9) + 2) / 5 + day - 1; /* 0 to 365 */
    long day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    return era * 146097 + day_of_era - 719468;
}

time_t mktime(struct tm *t)
{
    /* Months out of range first, carrying into years; then everything
     * else is just seconds, which may be any number. */
    long month = t->tm_mon;
    long year = t->tm_year + 1900L + month / 12;
    month %= 12;
    if (month < 0) {
        month += 12;
        year--;
    }
    long days = days_from_civil(year, (int)month + 1, 1) + t->tm_mday - 1;
    time_t seconds = days * 86400 + t->tm_hour * 3600L + t->tm_min * 60L + t->tm_sec;
    gmtime_r(&seconds, t);
    return seconds;
}

double difftime(time_t end, time_t start)
{
    return (double)(end - start);
}

/* ---- the calendar as text ---- */

static const char *const day_names[7] = {
    "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday",
};
static const char *const month_names[12] = {
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
};

/* The ISO 8601 week-numbering year and week (%G, %V): weeks start on
 * Monday, and week 1 is the one with the year's first Thursday in it.
 * So 1 January can be in week 52 or 53 of the year before, and 31
 * December in week 1 of the next. */
static void iso_week(const struct tm *t, int *iso_year, int *week)
{
    int year = t->tm_year + 1900;
    int weekday = (t->tm_wday + 6) % 7; /* Monday 0 */
    /* The Thursday of this week: its year is the ISO year. */
    int thursday = t->tm_yday - weekday + 3;
    int days_in_year = leap(year) ? 366 : 365;
    if (thursday < 0) {
        year--;
        thursday += leap(year) ? 366 : 365;
    } else if (thursday >= days_in_year) {
        thursday -= days_in_year;
        year++;
    }
    *iso_year = year;
    *week = thursday / 7 + 1;
}

size_t strftime(char *to, size_t size, const char *format, const struct tm *t)
{
    size_t n = 0;
    char piece[64];
    for (const char *f = format; *f; f++) {
        const char *text = piece;
        if (*f != '%') {
            piece[0] = *f;
            piece[1] = '\0';
        } else {
            f++;
            /* E and O ask for a locale's other era or digits; "C" has
             * none, so they change nothing. */
            if (*f == 'E' || *f == 'O')
                f++;
            int hour12 = t->tm_hour % 12 == 0 ? 12 : t->tm_hour % 12;
            int iso_year, week;
            switch (*f) {
            case 'a': snprintf(piece, sizeof piece, "%.3s", day_names[t->tm_wday % 7]); break;
            case 'A': text = day_names[t->tm_wday % 7]; break;
            case 'b': case 'h': snprintf(piece, sizeof piece, "%.3s", month_names[t->tm_mon % 12]); break;
            case 'B': text = month_names[t->tm_mon % 12]; break;
            case 'c':
                snprintf(piece, sizeof piece, "%.3s %.3s %2d %02d:%02d:%02d %d", day_names[t->tm_wday % 7],
                         month_names[t->tm_mon % 12], t->tm_mday, t->tm_hour, t->tm_min, t->tm_sec,
                         t->tm_year + 1900);
                break;
            case 'C': snprintf(piece, sizeof piece, "%02d", (t->tm_year + 1900) / 100); break;
            case 'd': snprintf(piece, sizeof piece, "%02d", t->tm_mday); break;
            case 'D': case 'x':
                snprintf(piece, sizeof piece, "%02d/%02d/%02d", t->tm_mon + 1, t->tm_mday, (t->tm_year + 1900) % 100);
                break;
            case 'e': snprintf(piece, sizeof piece, "%2d", t->tm_mday); break;
            case 'F': snprintf(piece, sizeof piece, "%d-%02d-%02d", t->tm_year + 1900, t->tm_mon + 1, t->tm_mday); break;
            case 'g': iso_week(t, &iso_year, &week); snprintf(piece, sizeof piece, "%02d", iso_year % 100); break;
            case 'G': iso_week(t, &iso_year, &week); snprintf(piece, sizeof piece, "%d", iso_year); break;
            case 'H': snprintf(piece, sizeof piece, "%02d", t->tm_hour); break;
            case 'I': snprintf(piece, sizeof piece, "%02d", hour12); break;
            case 'j': snprintf(piece, sizeof piece, "%03d", t->tm_yday + 1); break;
            case 'm': snprintf(piece, sizeof piece, "%02d", t->tm_mon + 1); break;
            case 'M': snprintf(piece, sizeof piece, "%02d", t->tm_min); break;
            case 'n': text = "\n"; break;
            case 'p': text = t->tm_hour < 12 ? "AM" : "PM"; break;
            case 'r':
                snprintf(piece, sizeof piece, "%02d:%02d:%02d %s", hour12, t->tm_min, t->tm_sec, t->tm_hour < 12 ? "AM" : "PM");
                break;
            case 'R': snprintf(piece, sizeof piece, "%02d:%02d", t->tm_hour, t->tm_min); break;
            case 'S': snprintf(piece, sizeof piece, "%02d", t->tm_sec); break;
            case 't': text = "\t"; break;
            case 'T': case 'X': snprintf(piece, sizeof piece, "%02d:%02d:%02d", t->tm_hour, t->tm_min, t->tm_sec); break;
            case 'u': snprintf(piece, sizeof piece, "%d", t->tm_wday == 0 ? 7 : t->tm_wday); break;
            /* Weeks starting on Sunday (U) or Monday (W); days before
             * the first such day are week 0. */
            case 'U': snprintf(piece, sizeof piece, "%02d", (t->tm_yday + 7 - t->tm_wday) / 7); break;
            case 'V': iso_week(t, &iso_year, &week); snprintf(piece, sizeof piece, "%02d", week); break;
            case 'w': snprintf(piece, sizeof piece, "%d", t->tm_wday); break;
            case 'W': snprintf(piece, sizeof piece, "%02d", (t->tm_yday + 7 - (t->tm_wday + 6) % 7) / 7); break;
            case 'y': snprintf(piece, sizeof piece, "%02d", (t->tm_year + 1900) % 100); break;
            case 'Y': snprintf(piece, sizeof piece, "%d", t->tm_year + 1900); break;
            case 'z': text = "+0000"; break; /* no time zones: always UTC */
            case 'Z': text = "UTC"; break;
            case '%': text = "%"; break;
            case '\0': f--; text = ""; break; /* a % at the very end */
            default: /* not a conversion: written as it is */
                snprintf(piece, sizeof piece, "%%%c", *f);
                break;
            }
        }
        size_t length = strlen(text);
        if (n + length >= size)
            return 0;
        memcpy(to + n, text, length);
        n += length;
    }
    if (size == 0)
        return 0;
    to[n] = '\0';
    return n;
}

char *asctime(const struct tm *t)
{
    static char text[32];
    if (strftime(text, sizeof text, "%c\n", t) == 0)
        text[0] = '\0';
    return text;
}

char *ctime(const time_t *when)
{
    return asctime(localtime(when));
}
