/* netinet/in.h - the address of a port on an IPv4 machine. */
#ifndef _NETINET_IN_H
#define _NETINET_IN_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdint.h>
#include <sys/socket.h>

typedef uint16_t in_port_t;
typedef uint32_t in_addr_t;

struct in_addr {
    in_addr_t s_addr; /* in network order: 10.0.2.15 is the bytes 10, 0, 2, 15 */
};

/* As Linux lays it out: the family, the port and the address - both of
 * those in network order (htons, htonl: arpa/inet.h) - and padding to
 * the size of a struct sockaddr. */
struct sockaddr_in {
    sa_family_t sin_family;
    in_port_t sin_port;
    struct in_addr sin_addr;
    unsigned char sin_zero[8];
};

#define INADDR_ANY ((in_addr_t)0x00000000)
#define INADDR_LOOPBACK ((in_addr_t)0x7f000001)
#define INADDR_NONE ((in_addr_t)0xffffffff)

#define IPPROTO_TCP 6
#define IPPROTO_UDP 17

#ifdef __cplusplus
}
#endif

#endif
