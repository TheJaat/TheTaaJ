/* inet.c - numbers in network order, and IPv4 addresses as text
 * (arpa/inet.h).
 *
 * Pure arithmetic: nothing here asks the system for anything, so the
 * file is also compiled for the host and every answer compared with
 * the host's own (inet_test.c, `make libc-test`). */

#include <arpa/inet.h>
#include <stddef.h>

/* Turning a number round does not depend on which way round this CPU
 * keeps it: the bytes are taken by arithmetic, not from memory. Going
 * there and coming back are the same operation. Written for a CPU that
 * keeps the least significant byte first - every CPU here. */
uint16_t htons(uint16_t value)
{
    return (uint16_t)((value >> 8) | (value << 8));
}

uint32_t htonl(uint32_t value)
{
    return (value >> 24) | ((value >> 8) & 0xff00) | ((value << 8) & 0xff0000) | (value << 24);
}

uint16_t ntohs(uint16_t value)
{
    return htons(value);
}

uint32_t ntohl(uint32_t value)
{
    return htonl(value);
}

/* Four decimal numbers from 0 to 255 with dots between them and
 * nothing else, none with a 0 in front: the bytes, in order. (The C library on Linux also
 * reads older shorter forms - "10.2" for 10.0.0.2 - and octal and
 * hex; inet_pton does not, and nor does this. A program that writes
 * an address writes it this way.) */
static int parse(const char *text, unsigned char bytes[4])
{
    for (int part = 0; part < 4; part++) {
        if (part > 0 && *text++ != '.')
            return 0;
        if (*text < '0' || *text > '9')
            return 0;
        unsigned value = 0;
        int digits = 0;
        while (*text >= '0' && *text <= '9') {
            /* No 0 in front of another digit: older readers took
             * "010" for octal - eight - and newer ones for ten. Text
             * two programs would read differently is no address. */
            if (digits == 1 && value == 0)
                return 0;
            value = value * 10 + (unsigned)(*text++ - '0');
            if (++digits > 3 || value > 255)
                return 0;
        }
        bytes[part] = (unsigned char)value;
    }
    return *text == '\0';
}

int inet_pton(int family, const char *text, void *address)
{
    if (family != AF_INET)
        return -1;
    unsigned char bytes[4];
    if (text == NULL || !parse(text, bytes))
        return 0;
    unsigned char *out = address;
    for (int n = 0; n < 4; n++)
        out[n] = bytes[n];
    return 1;
}

int inet_aton(const char *text, struct in_addr *address)
{
    struct in_addr parsed;
    if (inet_pton(AF_INET, text, &parsed) != 1)
        return 0;
    if (address != NULL)
        *address = parsed;
    return 1;
}

in_addr_t inet_addr(const char *text)
{
    struct in_addr parsed;
    return inet_aton(text, &parsed) ? parsed.s_addr : INADDR_NONE;
}

const char *inet_ntop(int family, const void *address, char *out, socklen_t size)
{
    if (family != AF_INET || size < INET_ADDRSTRLEN)
        return NULL;
    const unsigned char *bytes = address;
    char *at = out;
    for (int n = 0; n < 4; n++) {
        if (n > 0)
            *at++ = '.';
        unsigned value = bytes[n];
        if (value >= 100)
            *at++ = (char)('0' + value / 100);
        if (value >= 10)
            *at++ = (char)('0' + value / 10 % 10);
        *at++ = (char)('0' + value % 10);
    }
    *at = '\0';
    return out;
}

char *inet_ntoa(struct in_addr address)
{
    static char text[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &address, text, sizeof text);
    return text;
}
