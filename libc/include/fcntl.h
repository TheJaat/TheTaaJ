/* fcntl.h - opening a file. */
#ifndef _FCNTL_H
#define _FCNTL_H

/* One of these: what the descriptor is for. */
#define O_RDONLY 0      /* reading */
#define O_WRONLY 1      /* writing */
#define O_RDWR   2      /* both */

/* And any of these, joined on with |. */
#define O_CREAT  0100   /* make the file if it is not there */
#define O_EXCL   0200   /* with O_CREAT: fail if it is there */
#define O_TRUNC  01000  /* empty it first */
#define O_APPEND 02000  /* every write goes at the end */

/* Opens the file at path. Returns a descriptor - the lowest number
 * not in use - or -1, with the reason in errno.
 *
 * A third argument, the permissions of a file O_CREAT makes, is
 * accepted and not used: files here have no permissions. What a
 * program may do to files is decided by what it was started with
 * (`with files`, `with readonly`), not by the file. */
int open(const char *path, int flags, ...);

#endif
