/* sys/types.h - the types the system's functions use. */
#ifndef _SYS_TYPES_H
#define _SYS_TYPES_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stddef.h>

/* A size that can also say "it failed": -1. */
typedef long ssize_t;

/* A place in a file, or a distance between two: bytes, with a sign. */
typedef long off_t;

/* A process's number (getpid). */
typedef int pid_t;

/* What stat (sys/stat.h) says about a file. TheTaaJ's files have no
 * owners, permissions or links yet; the types are here for the fields
 * that will say so, and are 0 or made up (sys/stat.h says which). */
typedef unsigned int uid_t;
typedef unsigned int gid_t;
typedef unsigned int mode_t;
typedef unsigned long dev_t;
typedef unsigned long ino_t;
typedef unsigned long nlink_t;
typedef long blksize_t;
typedef long blkcnt_t;

#ifdef __cplusplus
}
#endif

#endif
