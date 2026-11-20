/* chello - the first C program for TheTaaJ.
 *
 * It was compiled on another machine, by a compiler that has never
 * heard of this system, and copied to the disk as a file. It shows the
 * three things a C program is given before it has done anything: its
 * arguments, somewhere to write, and a way to end.
 *
 *   chello [WORD...]     says hello, and what it was given
 *   chello fail          the same, and exits with code 3
 *   chello exit          the same, by calling exit(4)
 *   chello float         the same, and a sum with a fraction in it
 *
 * There is no printf yet, so it writes its numbers itself. */

#include <stdlib.h>
#include <unistd.h>

static size_t length(const char *text)
{
    size_t n = 0;
    while (text[n] != '\0')
        n++;
    return n;
}

static void print(const char *text)
{
    write(STDOUT_FILENO, text, length(text));
}

static void print_number(int value)
{
    char digits[12];
    int at = sizeof digits;
    do {
        digits[--at] = (char)('0' + value % 10);
        value /= 10;
    } while (value > 0);
    write(STDOUT_FILENO, digits + at, sizeof digits - (size_t)at);
}

static int same(const char *a, const char *b)
{
    while (*a != '\0' && *a == *b) {
        a++;
        b++;
    }
    return *a == *b;
}

int main(int argc, char **argv)
{
    print("Hello from C\n");
    print("argc ");
    print_number(argc);
    print("\n");
    for (int n = 1; n < argc; n++) {
        print("argv[");
        print_number(n);
        print("] ");
        print(argv[n]);
        print("\n");
    }
    /* The array ends in a null pointer: C promises it. */
    if (argv[argc] != NULL)
        print("argv does not end in NULL\n");
    if (argc > 1 && same(argv[1], "float")) {
        /* The kernel does not save the floating-point registers when it
         * switches programs, so this is compiled to use none: the
         * division and the multiplication are calls to functions that
         * do them with whole numbers (soft float). `volatile` keeps the
         * compiler from working the answer out itself. */
        volatile double seven = 7.0;
        volatile int two = 2;
        double result = seven / two * 4;
        print("7.0 / 2 * 4 = ");
        print_number((int)result);
        print("\n");
    }
    if (argc > 1 && same(argv[1], "fail"))
        return 3;
    if (argc > 1 && same(argv[1], "exit"))
        exit(4);
    return 0;
}
