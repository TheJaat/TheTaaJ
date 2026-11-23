/* utime.h - utime: marks a file changed.
 *
 * With times null, the file gets a new, higher change stamp, as touch
 * does; with times, utime fails with EPERM: TheTaaJ keeps a stamp for
 * each file, not a time that can be set (sys/stat.h). */

#ifndef _UTIME_H
#define _UTIME_H

#include <sys/types.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

struct utimbuf {
    time_t actime;
    time_t modtime;
};

int utime(const char *path, const struct utimbuf *times);

#ifdef __cplusplus
}
#endif

#endif
