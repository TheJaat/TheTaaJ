/* users.c - users and groups, of which TheTaaJ has none (pwd.h says
 * why): no user is found, and every program's user and group are 0.
 * And the size of the file system (sys/statvfs.h), not known yet. */

#include <pwd.h>
#include <stddef.h>
#include <unistd.h>

struct passwd *getpwnam(const char *name)
{
    (void)name;
    return NULL;
}

struct passwd *getpwuid(uid_t uid)
{
    (void)uid;
    return NULL;
}

int getpwnam_r(const char *name, struct passwd *entry, char *buffer, size_t size, struct passwd **result)
{
    (void)name;
    (void)entry;
    (void)buffer;
    (void)size;
    *result = NULL;
    return 0;
}

int getpwuid_r(uid_t uid, struct passwd *entry, char *buffer, size_t size, struct passwd **result)
{
    (void)uid;
    (void)entry;
    (void)buffer;
    (void)size;
    *result = NULL;
    return 0;
}

uid_t getuid(void)
{
    return 0;
}

uid_t geteuid(void)
{
    return 0;
}

gid_t getgid(void)
{
    return 0;
}

gid_t getegid(void)
{
    return 0;
}

/* ---- the file system's size (sys/statvfs.h): not known yet ---- */

#include <errno.h>
#include <sys/statvfs.h>

int statvfs(const char *path, struct statvfs *out)
{
    (void)path;
    (void)out;
    errno = ENOSYS;
    return -1;
}

int fstatvfs(int fd, struct statvfs *out)
{
    (void)fd;
    (void)out;
    errno = ENOSYS;
    return -1;
}
