/* sys/socket.h - sockets: a descriptor that is one end of a connection
 * across the network.
 *
 * The functions are in libc.a (libc/src/descriptors.rs). A socket is a
 * descriptor like a file's: read, write and close work on it. What is
 * here so far is TCP over IPv4 - a stream - and nothing else:
 *
 *   a client                     a server
 *     s = socket(AF_INET,          s = socket(AF_INET, SOCK_STREAM, 0)
 *           SOCK_STREAM, 0)        bind(s, address with the port)
 *     connect(s, address)          listen(s, 4)
 *     write(s, ...)                c = accept(s, NULL, NULL)
 *     read(s, ...)                 read(c, ...), write(c, ...)
 *     close(s)                     close(c), and accept again
 *
 * Each returns -1 when it fails, with the reason in errno. The numbers
 * are the ones Linux uses. */
#ifndef _SYS_SOCKET_H
#define _SYS_SOCKET_H

#ifdef __cplusplus
extern "C" {
#endif

#include <sys/types.h>

typedef unsigned int socklen_t;
typedef unsigned short sa_family_t;

/* An address of any kind: what the functions take, so that one
 * function serves every kind. The real thing is a struct sockaddr_in
 * (netinet/in.h), and a pointer to it is cast to this. */
struct sockaddr {
    sa_family_t sa_family;
    char sa_data[14];
};

#define AF_UNIX 1      /* names in the file system: not here (socket fails) */
#define AF_LOCAL AF_UNIX
#define AF_INET 2      /* IPv4 - the only family here */
#define PF_INET AF_INET
#define SOCK_STREAM 1  /* a connection: TCP */
#define SOCK_DGRAM 2   /* datagrams: UDP (not from C yet) */

/* The one option there is: asked for by most servers, so that a port
 * can be listened on again at once. Here that is always so. */
#define SOL_SOCKET 1
#define SO_REUSEADDR 2

/* What shutdown stops. */
#define SHUT_RD 0
#define SHUT_WR 1
#define SHUT_RDWR 2

/* A socket: a new descriptor, not yet connected to anything. */
int socket(int family, int type, int protocol);

/* Connects to the address (a struct sockaddr_in), and waits until the
 * connection is made - half a minute at most, if nothing answers. */
int connect(int socket, const struct sockaddr *address, socklen_t length);

/* Says which port of this machine the socket is to listen on. */
int bind(int socket, const struct sockaddr *address, socklen_t length);

/* Listens: from now on others can connect to the port. (How many may
 * wait is the network service's to decide; `backlog` is ignored.) */
int listen(int socket, int backlog);

/* Waits for a connection to the port and returns a new descriptor for
 * it. If `address` is not NULL, who connected is written there. */
int accept(int socket, struct sockaddr *address, socklen_t *length);

/* read and write, with flags - of which there are none yet: 0. */
ssize_t send(int socket, const void *buffer, size_t length, int flags);
ssize_t recv(int socket, void *buffer, size_t length, int flags);

/* SHUT_WR: nothing more will be written - the other side reads to the
 * end. Reading goes on. */
int shutdown(int socket, int how);

int setsockopt(int socket, int level, int option, const void *value, socklen_t length);

/* The port of this machine a socket uses: after bind with port 0, or
 * after connect, which picks one. */
int getsockname(int socket, struct sockaddr *address, socklen_t *length);

#ifdef __cplusplus
}
#endif

#endif
