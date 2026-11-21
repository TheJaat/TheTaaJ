/* inet_test.c - inet.c against this machine's own.
 *
 * Runs on the host. inet.c is compiled for the host and its names
 * changed (host_names: inet_pton to taaj_inet_pton), so that both sit
 * in one program. Then:
 *
 *   htons ... ntohl   every 16-bit value, and many 32-bit ones
 *   inet_ntop         every value of each byte
 *   inet_pton         texts chosen to be awkward; and the text of
 *                     every address inet_ntop wrote must come back as
 *                     the same address
 *
 * Built and run by `make libc-test`. */

#include <arpa/inet.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

uint16_t taaj_htons(uint16_t value);
uint32_t taaj_htonl(uint32_t value);
uint16_t taaj_ntohs(uint16_t value);
uint32_t taaj_ntohl(uint32_t value);
in_addr_t taaj_inet_addr(const char *text);
int taaj_inet_aton(const char *text, struct in_addr *address);
char *taaj_inet_ntoa(struct in_addr address);
int taaj_inet_pton(int family, const char *text, void *address);
const char *taaj_inet_ntop(int family, const void *address, char *out, socklen_t size);

static int checks, failures;

static void check(int passed, const char *what, const char *text)
{
    checks++;
    if (!passed) {
        failures++;
        if (failures < 20)
            fprintf(stderr, "inet: %s: %s\n", what, text);
    }
}

int main(void)
{
    for (uint32_t value = 0; value <= 0xffff; value++) {
        check(taaj_htons((uint16_t)value) == htons((uint16_t)value), "htons", "");
        check(taaj_ntohs((uint16_t)value) == ntohs((uint16_t)value), "ntohs", "");
    }
    for (uint64_t value = 0; value <= 0xffffffffu; value += 65537) {
        check(taaj_htonl((uint32_t)value) == htonl((uint32_t)value), "htonl", "");
        check(taaj_ntohl((uint32_t)value) == ntohl((uint32_t)value), "ntohl", "");
    }

    /* Each byte through every value, the others fixed. */
    for (int place = 0; place < 4; place++) {
        for (int value = 0; value < 256; value++) {
            unsigned char bytes[4] = {10, 0, 2, 15};
            bytes[place] = (unsigned char)value;
            char ours[INET_ADDRSTRLEN], theirs[INET_ADDRSTRLEN];
            check(taaj_inet_ntop(AF_INET, bytes, ours, sizeof ours) == ours, "inet_ntop returns its buffer", "");
            inet_ntop(AF_INET, bytes, theirs, sizeof theirs);
            check(strcmp(ours, theirs) == 0, "inet_ntop", theirs);
            unsigned char back[4] = {0};
            check(taaj_inet_pton(AF_INET, ours, back) == 1 && memcmp(back, bytes, 4) == 0, "inet_pton of inet_ntop", ours);
            struct in_addr address;
            memcpy(&address, bytes, 4);
            check(strcmp(taaj_inet_ntoa(address), theirs) == 0, "inet_ntoa", theirs);
        }
    }
    char small[INET_ADDRSTRLEN - 1];
    unsigned char biggest[4] = {255, 255, 255, 255};
    check(taaj_inet_ntop(AF_INET, biggest, small, sizeof small) == NULL, "inet_ntop with too little room", "");
    check(taaj_inet_ntop(10, biggest, small, sizeof small) == NULL, "inet_ntop of a family not known", "");

    /* Texts that are, and are not, addresses - as this machine's
     * inet_pton reads them (which is the strict reading). */
    const char *texts[] = {
        "10.0.2.2", "0.0.0.0", "255.255.255.255", "1.2.3.4", "010.0.2.2", "10.0.2.256", "10.0.2", "10.0.2.2.",
        "10.0.2.2.1", ".10.0.2.2", "10..2.2", "10.0.2.2 ", " 10.0.2.2", "10.0.2.-1", "10.0.2.0x2", "1000.0.2.2",
        "", "a.b.c.d", "10.0.2.2\n", "0255.0.0.1", "00.0.0.0", "999.0.0.0", "1.2.3.4x",
    };
    for (size_t n = 0; n < sizeof texts / sizeof texts[0]; n++) {
        unsigned char ours[4] = {0}, theirs[4] = {0};
        int a = taaj_inet_pton(AF_INET, texts[n], ours), b = inet_pton(AF_INET, texts[n], theirs);
        check(a == b && memcmp(ours, theirs, 4) == 0, "inet_pton", texts[n]);
        struct in_addr address;
        check(taaj_inet_aton(texts[n], &address) == a, "inet_aton says the same as inet_pton", texts[n]);
        if (a == 1)
            check(taaj_inet_addr(texts[n]) == *(in_addr_t *)theirs, "inet_addr", texts[n]);
        else
            check(taaj_inet_addr(texts[n]) == INADDR_NONE, "inet_addr of no address", texts[n]);
    }
    check(taaj_inet_pton(10, "10.0.2.2", small) == -1, "inet_pton of a family not known", "");

    if (failures) {
        fprintf(stderr, "inet-test: %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("inet-test: all %d checks passed\n", checks);
    return 0;
}
