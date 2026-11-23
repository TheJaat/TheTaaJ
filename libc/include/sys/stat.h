/* sys/stat.h - what a file is: stat, fstat, lstat; and mkdir.
 *
 * TheTaaJ's file system keeps a name, a kind (file or directory), a
 * size and a change stamp for each file, and nothing else yet: no
 * owner, permissions, times of day or links. So of struct stat:
 *
 *   st_mode    S_IFREG or S_IFDIR, and permissions 0644 or 0755 - what
 *              the file service would let a program given the files do
 *              (the service, not the bits, decides that)
 *   st_size    the size in bytes
 *   st_ino     a number for the file, made from its whole path: two
 *              paths to the same file give the same number, and two
 *              files different ones (unless their paths' 64-bit hashes
 *              collide). There are no hard links, so a path is a file.
 *   st_dev     1 for every file: there is one file system
 *   st_nlink   1
 *   st_blksize 4096, and st_blocks the size in 512-byte blocks
 *   the times  all three the file's change stamp, in seconds: a number
 *              that only grows, given by the file system to each change
 *              (the generation of the commit that made it). Not a time
 *              of day; but of two files the one changed later has the
 *              higher st_mtime, which is what make asks. Never 0 (make
 *              would think the file missing): a file not changed since
 *              its disk began stamping is 1, a directory 1 too.
 *   st_uid, st_gid  0
 *
 * Standard input, output and error are character devices (S_IFCHR). */

#ifndef _SYS_STAT_H
#define _SYS_STAT_H

#include <sys/types.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

struct stat {
    dev_t st_dev;
    ino_t st_ino;
    nlink_t st_nlink;
    mode_t st_mode;
    uid_t st_uid;
    gid_t st_gid;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    struct timespec st_atim;
    struct timespec st_mtim;
    struct timespec st_ctim;
};

#define st_atime st_atim.tv_sec
#define st_mtime st_mtim.tv_sec
#define st_ctime st_ctim.tv_sec

/* The kind, in the top bits of st_mode. */
#define S_IFMT 0170000
#define S_IFSOCK 0140000
#define S_IFLNK 0120000
#define S_IFREG 0100000
#define S_IFBLK 0060000
#define S_IFDIR 0040000
#define S_IFCHR 0020000
#define S_IFIFO 0010000

#define S_ISREG(m) (((m) & S_IFMT) == S_IFREG)
#define S_ISDIR(m) (((m) & S_IFMT) == S_IFDIR)
#define S_ISCHR(m) (((m) & S_IFMT) == S_IFCHR)
#define S_ISBLK(m) (((m) & S_IFMT) == S_IFBLK)
#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)
#define S_ISLNK(m) (((m) & S_IFMT) == S_IFLNK)
#define S_ISSOCK(m) (((m) & S_IFMT) == S_IFSOCK)

/* The permissions, in the low bits. */
#define S_ISUID 04000
#define S_ISGID 02000
#define S_ISVTX 01000
#define S_IRWXU 0700
#define S_IRUSR 0400
#define S_IWUSR 0200
#define S_IXUSR 0100
#define S_IRWXG 070
#define S_IRGRP 040
#define S_IWGRP 020
#define S_IXGRP 010
#define S_IRWXO 07
#define S_IROTH 04
#define S_IWOTH 02
#define S_IXOTH 01

int stat(const char *path, struct stat *out);
int fstat(int fd, struct stat *out);
/* There are no symbolic links: lstat is stat. */
int lstat(const char *path, struct stat *out);

/* Makes a directory. The mode is not kept (there are no permissions). */
int mkdir(const char *path, mode_t mode);

/* Permissions are not kept: these change nothing and succeed for a
 * file that is there (chmod, fchmod), and umask says 022. */
/* Marking a file changed (as touch does). Only the change time is
 * kept, as a stamp, and it can be made newer but not set: times null,
 * or times[1].tv_nsec UTIME_NOW, gives the file a new, higher stamp;
 * UTIME_OMIT leaves it; a time given as a time fails with EPERM. The
 * access time is not kept. utimensat's dirfd must be AT_FDCWD (or the
 * path from the root); its flags mean nothing (there are no links). */
#define UTIME_NOW ((1l << 30) - 1l)
#define UTIME_OMIT ((1l << 30) - 2l)
int utimensat(int dirfd, const char *path, const struct timespec times[2], int flags);
int futimens(int fd, const struct timespec times[2]);

int chmod(const char *path, mode_t mode);
int fchmod(int fd, mode_t mode);
mode_t umask(mode_t mask);

#ifdef __cplusplus
}
#endif

#endif
