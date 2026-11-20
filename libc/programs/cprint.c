/* cprint - printf, used from C.
 *
 *   cprint [WORD...]     prints lines with printf, and its arguments
 *   cprint long          one printf of more than its buffer holds
 *
 * What printf makes of every kind of format is checked on the host,
 * against the host's own (libc/c/format_test.c). This checks the other
 * half, which only a running program can: that the text arrives - on
 * the screen, in a file, down a pipe - whole and in order. */

#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "long") == 0) {
        /* 600 bytes and a new line from one call: more than twice what
         * printf gathers before it writes, so it must write part-way
         * through and lose nothing. `cprint long | wc` counts them. */
        char run[601];
        memset(run, 'x', 600);
        run[600] = '\0';
        printf("%s\n", run);
        return 0;
    }

    int lines = 0;
    lines += printf("printf: %d %d %x %#X [%5s] [%-5s] [%05d] %c 100%%\n", 42, -7, 255, 42, "hi", "hi", 42, 'x') > 0;
    lines += printf("sizes: %zu %lld\n", SIZE_MAX, LLONG_MIN) > 0;

    /* What printf returns is how many bytes it wrote. */
    int count = printf("12345\n");
    lines++;
    lines += printf("printf returned %d\n", count) > 0;

    /* snprintf: never more than the buffer, and it says how long the
     * whole text would have been. */
    char small[8];
    int wanted = snprintf(small, sizeof small, "%s, %s", "hello", "world");
    lines += printf("snprintf: wanted %d, kept <%s>\n", wanted, small) > 0;

    for (int n = 1; n < argc; n++)
        lines += printf("argument %d of %d: %s\n", n, argc - 1, argv[n]) > 0;

    /* The plain ones. */
    lines += puts("puts: a line of its own") >= 0;
    fputs("fputs and ", stdout);
    putchar('p');
    putc('u', stdout);
    fputc('t', stdout);
    fputs("char\n", stdout);
    lines++;

    /* And the other stream: not part of the output. */
    fprintf(stderr, "cprint: %d lines went to standard output\n", lines);
    return 0;
}
