/* pwd.h - the users of the system, as Unix keeps them in /etc/passwd.
 *
 * TheTaaJ has no users yet: who may do what is said by the handles a
 * program is given, not by whose program it is. So no user is ever
 * found: getpwnam and getpwuid return NULL, and their _r forms return
 * 0 with *result set to NULL - POSIX's "no such user". */

#ifndef _PWD_H
#define _PWD_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct passwd {
    char *pw_name;
    char *pw_passwd;
    uid_t pw_uid;
    gid_t pw_gid;
    char *pw_gecos;
    char *pw_dir;
    char *pw_shell;
};

struct passwd *getpwnam(const char *name);
struct passwd *getpwuid(uid_t uid);
int getpwnam_r(const char *name, struct passwd *entry, char *buffer, size_t size, struct passwd **result);
int getpwuid_r(uid_t uid, struct passwd *entry, char *buffer, size_t size, struct passwd **result);

#ifdef __cplusplus
}
#endif

#endif
