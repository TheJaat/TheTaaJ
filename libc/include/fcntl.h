/* fcntl.h - opening a file. */
#ifndef _FCNTL_H
#define _FCNTL_H

#ifdef __cplusplus
extern "C" {
#endif

/* One of these: what the descriptor is for. */
#define O_RDONLY 0      /* reading */
#define O_WRONLY 1      /* writing */
#define O_RDWR   2      /* both */

/* And any of these, joined on with |. */
#define O_CREAT  0100   /* make the file if it is not there */
#define O_EXCL   0200   /* with O_CREAT: fail if it is there */
#define O_TRUNC  01000  /* empty it first */
#define O_APPEND 02000  /* every write goes at the end */

/* Accepted, and meaning nothing more here: a program cannot replace
 * itself with another (exec), so there is nothing to close on it;
 * files never make a program wait; there are no links to follow. */
#define O_NONBLOCK  04000
#define O_NDELAY    O_NONBLOCK
#define O_DIRECTORY 0200000
#define O_NOFOLLOW  0400000
#define O_CLOEXEC   02000000
#define O_NOCTTY    0400

/* Opens the file at path. Returns a descriptor - the lowest number
 * not in use - or -1, with the reason in errno.
 *
 * A third argument, the permissions of a file O_CREAT makes, is
 * accepted and not used: files here have no permissions. What a
 * program may do to files is decided by what it was started with
 * (`with files`, `with readonly`), not by the file. */
int open(const char *path, int flags, ...);

/* fcntl(fd, command, ...): asks about or changes a descriptor.
 *
 *   F_DUPFD n         dup, to the lowest free number from n up
 *   F_GETFD, F_SETFD  FD_CLOEXEC: kept and given back, though nothing
 *                     execs here
 *   F_GETFL           the flags it was opened with
 *   F_SETFL           O_APPEND and O_NONBLOCK on or off (the others
 *                     cannot change, as POSIX says)
 *   F_GETLK, F_SETLK, F_SETLKW
 *                     record locks: TheTaaJ keeps no locks between
 *                     programs, so these fail with ENOLCK - POSIX's "no
 *                     locks available" - rather than lock nothing */
#define F_DUPFD 0
#define F_GETFD 1
#define F_SETFD 2
#define F_GETFL 3
#define F_SETFL 4
#define F_GETLK 5
#define F_SETLK 6
#define F_SETLKW 7
#define F_DUPFD_CLOEXEC 1030
#define FD_CLOEXEC 1

#define F_RDLCK 0
#define F_WRLCK 1
#define F_UNLCK 2

struct flock {
    short l_type;
    short l_whence;
    long l_start;
    long l_len;
    int l_pid;
};

int fcntl(int fd, int command, ...);

#ifdef __cplusplus
}
#endif

#endif
