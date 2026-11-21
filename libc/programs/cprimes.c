/* cprimes - counts the primes below a number, the slow way.
 *
 *   cprimes LIMIT
 *
 * Prints how many primes there are below LIMIT. It finds them by trial
 * division: for each number, try every possible divisor up to its
 * square root. There are far quicker ways (a sieve crosses out
 * multiples instead of dividing). Slow is the point: this program is
 * here to keep a CPU busy for a second or two, with an answer at the
 * end that is either right or wrong.
 *
 *   cprimes 100            25 primes
 *   cprimes 300000 &       ...in the background; start four, and four
 *   time wait              CPUs work at once
 *
 * Like cwords, it is standard C and nothing else, and the same file is
 * compiled for the machine TheTaaJ is built on. The automated test
 * runs both and compares what they print.
 *
 * Whole numbers only. The kernel does not yet save the floating-point
 * registers when it switches between programs, so a C program for
 * TheTaaJ must not use them - which is why the loop below asks
 * "divisor * divisor <= number" instead of calling sqrt. */

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>

/* Whether `number` is prime: nothing from 2 up to its square root
 * divides it. Even numbers are dealt with first, so that only odd
 * divisors need trying. */
static int is_prime(long number) {
    if (number < 2) {
        return 0;
    }
    if (number % 2 == 0) {
        return number == 2;
    }
    for (long divisor = 3; divisor * divisor <= number; divisor += 2) {
        if (number % divisor == 0) {
            return 0;
        }
    }
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: cprimes LIMIT\n");
        return 2;
    }
    /* strtol says where it stopped reading (`end`) and, through errno,
     * whether the number was too big for a long. A limit like "12x" or
     * "" is refused rather than read as 12 or 0. The upper bound keeps
     * divisor * divisor, above, inside a long with room to spare. */
    char *end;
    errno = 0;
    long limit = strtol(argv[1], &end, 10);
    if (errno != 0 || end == argv[1] || *end != '\0' || limit < 0 || limit > 1000000000L) {
        fprintf(stderr, "cprimes: not a limit between 0 and 1000000000: %s\n", argv[1]);
        return 2;
    }

    long count = 0;
    for (long number = 2; number < limit; number++) {
        count += is_prime(number);
    }
    printf("cprimes: %ld primes below %ld\n", count, limit);
    return 0;
}
