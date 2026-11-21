/* cnet - TCP from C, with the socket calls every Unix has.
 *
 *   cnet send ADDRESS PORT WORD...   connects, sends the words and a
 *                                    new line, says it is done
 *                                    (shutdown), and prints everything
 *                                    that comes back until the other
 *                                    side is done too
 *   cnet serve PORT [COUNT]          listens on PORT; to each
 *                                    connection it says how many bytes
 *                                    it was sent, in upper case what
 *                                    they were, and closes. After
 *                                    COUNT connections it ends.
 *
 * The same source compiles on Linux unchanged (try `cc cnet.c`): that
 * is the point of these calls. A program is given the network only by
 * whoever starts it - the shell gives it to this one. */

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int usage(void)
{
    fprintf(stderr, "usage: cnet send ADDRESS PORT WORD...\n       cnet serve PORT [COUNT]\n");
    return 2;
}

/* All of `count` bytes, however many writes that takes. */
static int write_all(int fd, const char *data, size_t count)
{
    while (count > 0) {
        ssize_t written = write(fd, data, count);
        if (written < 0)
            return -1;
        data += written;
        count -= (size_t)written;
    }
    return 0;
}

static int send_words(const char *address, int port, int count, char **words)
{
    struct sockaddr_in to;
    memset(&to, 0, sizeof to);
    to.sin_family = AF_INET;
    to.sin_port = htons((uint16_t)port);
    if (inet_pton(AF_INET, address, &to.sin_addr) != 1) {
        fprintf(stderr, "cnet: %s is not an address\n", address);
        return 2;
    }
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) {
        perror("cnet: socket");
        return 1;
    }
    if (connect(s, (struct sockaddr *)&to, sizeof to) < 0) {
        fprintf(stderr, "cnet: %s:%d: %s\n", address, port, strerror(errno));
        close(s);
        return 1;
    }
    struct sockaddr_in here;
    socklen_t length = sizeof here;
    if (getsockname(s, (struct sockaddr *)&here, &length) == 0)
        printf("cnet: connected to %s:%d from port %d\n", inet_ntoa(to.sin_addr), port, ntohs(here.sin_port));

    for (int n = 0; n < count; n++) {
        if (write_all(s, words[n], strlen(words[n])) < 0 || write_all(s, n + 1 < count ? " " : "\n", 1) < 0) {
            perror("cnet: write");
            close(s);
            return 1;
        }
    }
    if (shutdown(s, SHUT_WR) < 0) {
        perror("cnet: shutdown");
        close(s);
        return 1;
    }
    /* Everything until the end, as it comes. */
    char piece[512];
    ssize_t got;
    long total = 0;
    while ((got = read(s, piece, sizeof piece)) > 0) {
        fwrite(piece, 1, (size_t)got, stdout);
        total += got;
    }
    fflush(stdout);
    if (got < 0) {
        perror("cnet: read");
        close(s);
        return 1;
    }
    printf("cnet: %ld bytes back, and the other side is done\n", total);
    close(s);
    return 0;
}

static int serve(int port, long count)
{
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) {
        perror("cnet: socket");
        return 1;
    }
    int yes = 1;
    setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof yes);
    struct sockaddr_in here;
    memset(&here, 0, sizeof here);
    here.sin_family = AF_INET;
    here.sin_port = htons((uint16_t)port);
    here.sin_addr.s_addr = htonl(INADDR_ANY);
    if (bind(s, (struct sockaddr *)&here, sizeof here) < 0 || listen(s, 4) < 0) {
        fprintf(stderr, "cnet: port %d: %s\n", port, strerror(errno));
        close(s);
        return 1;
    }
    printf("cnet: listening on port %d\n", port);
    fflush(stdout);
    for (long served = 0; served < count; served++) {
        struct sockaddr_in from;
        socklen_t length = sizeof from;
        int c = accept(s, (struct sockaddr *)&from, &length);
        if (c < 0) {
            perror("cnet: accept");
            close(s);
            return 1;
        }
        /* Read to the end, keeping the first part of it. */
        char text[256];
        size_t kept = 0;
        long total = 0;
        char piece[512];
        ssize_t got;
        while ((got = read(c, piece, sizeof piece)) > 0) {
            for (ssize_t n = 0; n < got && kept + 1 < sizeof text; n++) {
                char ch = piece[n];
                text[kept++] = ch >= 'a' && ch <= 'z' ? (char)(ch - 'a' + 'A') : ch;
            }
            total += got;
        }
        text[kept] = '\0';
        char answer[400];
        int length_out = snprintf(answer, sizeof answer, "you sent %ld bytes: %s", total, text);
        if (got < 0 || write_all(c, answer, (size_t)length_out) < 0)
            fprintf(stderr, "cnet: %s:%d: %s\n", inet_ntoa(from.sin_addr), ntohs(from.sin_port), strerror(errno));
        else
            printf("cnet: %ld bytes from %s:%d, answered\n", total, inet_ntoa(from.sin_addr), ntohs(from.sin_port));
        fflush(stdout);
        close(c);
    }
    close(s);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc >= 5 && strcmp(argv[1], "send") == 0) {
        int port = atoi(argv[3]);
        if (port <= 0 || port > 65535)
            return usage();
        return send_words(argv[2], port, argc - 4, argv + 4);
    }
    if ((argc == 3 || argc == 4) && strcmp(argv[1], "serve") == 0) {
        int port = atoi(argv[2]);
        long count = argc == 4 ? atol(argv[3]) : 1000000000L;
        if (port <= 0 || port > 65535 || count <= 0)
            return usage();
        return serve(port, count);
    }
    return usage();
}
