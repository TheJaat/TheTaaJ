/* errno.h - why the last call failed.
 *
 * A function that fails returns a value that could not be an answer
 * (-1, or NULL) and leaves the reason here. Nothing sets errno back to
 * 0 when a call succeeds: it means something only straight after a
 * call that said it failed. strerror (string.h) and perror (stdio.h)
 * turn it into words.
 *
 * The numbers are the ones Linux uses. */
#ifndef _ERRNO_H
#define _ERRNO_H

extern int errno;
#define errno errno

#define ENOENT      2   /* no such file or directory */
#define EIO         5   /* the file service or the disk failed */
#define EBADF       9   /* not an open descriptor, or not open for that */
#define ENOMEM     12   /* out of memory */
#define EACCES     13   /* this program was not given the files */
#define EFAULT     14   /* a null pointer where an address was needed */
#define EEXIST     17   /* there is already something with that name */
#define ENOTDIR    20   /* not a directory */
#define EISDIR     21   /* is a directory */
#define EINVAL     22   /* not a valid argument */
#define EMFILE     24   /* too many open descriptors */
#define ENOTTY     25   /* not the screen */
#define EFBIG      27   /* too big for a file */
#define ENOSPC     28   /* the file system is full */
#define ESPIPE     29   /* a stream has no position to move */
#define EROFS      30   /* the files are read-only here */
#define ERANGE     34   /* a number too big to fit */
#define ENOSYS     38   /* TheTaaJ does not do that */
#define ENOTEMPTY  39   /* the directory is not empty */
#define EPIPE      32   /* the connection is closed: nothing more can be written */
#define ENOTSOCK   88   /* not a socket */
#define EDESTADDRREQ 89 /* the socket is not connected to anything */
#define ENOPROTOOPT 92  /* no such option */
#define EPROTONOSUPPORT 93 /* only TCP is spoken from C so far */
#define EOPNOTSUPP 95   /* a socket cannot do that */
#define EAFNOSUPPORT 97 /* only IPv4 addresses are known here */
#define EADDRINUSE 98   /* another program has that port */
#define ENETUNREACH 101 /* not on this network, and no way out of it */
#define ECONNRESET 104  /* the other side cut the connection off */
#define ENOBUFS   105   /* the network service has no room for more */
#define EISCONN   106   /* the socket is connected already */
#define ENOTCONN  107   /* the socket is not connected */
#define ETIMEDOUT 110   /* the service, or the other side, is not answering */
#define ECONNREFUSED 111 /* nothing is listening on that port there */
#define EHOSTUNREACH 113 /* nobody on this network has that address */

#endif
