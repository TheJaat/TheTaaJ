/* arpa/inet.h - numbers in network order, and addresses as text.
 *
 * A network sends the most significant byte of a number first ("big
 * endian", or network order). The CPUs here keep it last (little
 * endian). So a port or an address going into a struct sockaddr_in is
 * turned round first: htons - "host to network, short" - for a port,
 * htonl for an address held as a number; ntohs and ntohl back. */
#ifndef _ARPA_INET_H
#define _ARPA_INET_H

#ifdef __cplusplus
extern "C" {
#endif

#include <netinet/in.h>

uint16_t htons(uint16_t value);
uint32_t htonl(uint32_t value);
uint16_t ntohs(uint16_t value);
uint32_t ntohl(uint32_t value);

/* "10.0.2.2" as an address in network order; INADDR_NONE if it is not
 * one (which 255.255.255.255 also is - inet_aton can tell them apart). */
in_addr_t inet_addr(const char *text);

/* 1 if `text` is an address, written to *address; 0 if not. */
int inet_aton(const char *text, struct in_addr *address);

/* The address as text, in a buffer of the library's that the next call
 * writes over. */
char *inet_ntoa(struct in_addr address);

/* The same two for any family (only AF_INET here). inet_pton: 1 for an
 * address, 0 for text that is not one, -1 for a family not known.
 * inet_ntop: `out`, or NULL if it is shorter than INET_ADDRSTRLEN. */
#define INET_ADDRSTRLEN 16
int inet_pton(int family, const char *text, void *address);
const char *inet_ntop(int family, const void *address, char *out, socklen_t size);

#ifdef __cplusplus
}
#endif

#endif
