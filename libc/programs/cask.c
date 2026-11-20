/* cask - a C program that asks, and is answered from the keyboard.
 *
 *   cask        asks for a name and greets it; then adds up numbers,
 *               one a line, until the input ends (Ctrl+D), and prints
 *               the total
 *   cask one    reads one line and says what it was
 *
 * Its input need not be typed: `cask one < file` and `echo ada | cask
 * one` give it the same lines. It says which it had. */

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* One line of standard input, without the new line at its end. NULL
 * when there is no more. */
static char *line(char *room, int size)
{
    if (fgets(room, size, stdin) == NULL)
        return NULL;
    room[strcspn(room, "\n")] = '\0';
    return room;
}

int main(int argc, char **argv)
{
    char text[128];
    const char *how = isatty(STDIN_FILENO) ? "typed" : "from a file or another program";

    if (argc == 2 && strcmp(argv[1], "one") == 0) {
        if (line(text, sizeof text) == NULL) {
            printf("no line at all (%s)\n", how);
            return 1;
        }
        printf("one line: %s (%s)\n", text, how);
        return 0;
    }
    if (argc != 1) {
        fprintf(stderr, "usage: cask [one]\n");
        return 2;
    }

    /* No new line after the question: the answer is typed beside it.
     * It is seen all the same - reading standard input sends what
     * waits in standard output first. */
    printf("your name? ");
    if (line(text, sizeof text) == NULL) {
        printf("\nno name, then\n");
        return 1;
    }
    printf("hello, %s (%s)\n", text, how);

    printf("numbers to add, one a line; Ctrl+D ends\n");
    long total = 0;
    while (line(text, sizeof text) != NULL) {
        char *end;
        errno = 0;
        long number = strtol(text, &end, 10);
        if (end == text || *end != '\0' || errno == ERANGE) {
            fprintf(stderr, "cask: not a number: %s\n", text);
            continue;
        }
        total += number;
    }
    printf("total: %ld\n", total);
    return 0;
}
