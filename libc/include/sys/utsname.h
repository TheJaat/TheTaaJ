/* sys/utsname.h - uname: what system this is.
 *
 *   sysname   "TheTaaJ"
 *   nodename  "taaj", as gethostname says (there is no network name)
 *   release   "0.6", the era the system is in
 *   version   "TheTaaJ"
 *   machine   "x86_64" or "aarch64"
 *
 * The fields are 65 bytes, as Linux's are. */

#ifndef _SYS_UTSNAME_H
#define _SYS_UTSNAME_H

#ifdef __cplusplus
extern "C" {
#endif

struct utsname {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
};

int uname(struct utsname *out);

#ifdef __cplusplus
}
#endif

#endif
