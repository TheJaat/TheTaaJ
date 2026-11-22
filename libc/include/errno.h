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

#ifdef __cplusplus
extern "C" {
#endif

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

/* The rest of POSIX's, with Linux's numbers: nothing in TheTaaJ sets
 * these yet, but C++'s std::errc names every one. */
#define EPERM             1 /* not permitted */
#define ESRCH             3 /* no such process */
#define EINTR             4 /* interrupted */
#define ENXIO             6 /* no such device or address */
#define E2BIG             7 /* the arguments are too long */
#define ENOEXEC           8 /* not a program that can be run */
#define ECHILD           10 /* no child process */
#define EAGAIN           11 /* not now: try again */
#define EBUSY            16 /* in use */
#define EXDEV            18 /* not on the same file system */
#define ENODEV           19 /* no such device */
#define ENFILE           23 /* too many files open in the system */
#define ETXTBSY          26 /* a program that is running */
#define EMLINK           31 /* too many links */
#define EDOM             33 /* outside the function's domain */
#define EDEADLK          35 /* it would wait for itself forever */
#define ENAMETOOLONG     36 /* the name is too long */
#define ENOLCK           37 /* no lock to be had */
#define ELOOP            40 /* too many links followed */
#define ENOMSG           42 /* no message of that kind */
#define EIDRM            43 /* that identifier was removed */
#define ENOSTR           60 /* not a stream device */
#define ENODATA          61 /* no data */
#define ETIME            62 /* the timer ran out */
#define ENOSR            63 /* no stream resources */
#define ENOLINK          67 /* the link is gone */
#define EPROTO           71 /* the other side broke the protocol */
#define EMULTIHOP        72 /* too many hops */
#define EBADMSG          74 /* a message that makes no sense */
#define EOVERFLOW        75 /* too big for the type it goes in */
#define EILSEQ           84 /* not a valid character */
#define EMSGSIZE         90 /* the message is too long */
#define EPROTOTYPE       91 /* the wrong kind of socket for that protocol */
#define EADDRNOTAVAIL    99 /* that address is not this machine's */
#define ENETDOWN        100 /* the network is down */
#define ENETRESET       102 /* the network dropped the connection */
#define ECONNABORTED    103 /* the connection was given up */
#define EALREADY        114 /* already under way */
#define EINPROGRESS     115 /* under way, not finished */
#define ESTALE          116 /* a file that is no longer there */
#define EDQUOT          122 /* over the disk quota */
#define ECANCELED       125 /* cancelled */
#define EOWNERDEAD      130 /* its owner died holding it */
#define ENOTRECOVERABLE 131 /* broken past repair */
#define EWOULDBLOCK EAGAIN
#define ENOTSUP     EOPNOTSUPP

/* The largest of them (C++'s library asks). */
#define ELAST 131

#ifdef __cplusplus
}
#endif

#endif
