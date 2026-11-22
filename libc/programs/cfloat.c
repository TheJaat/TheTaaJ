/* cfloat.c - floating point in a C program, done in the FPU's
 * registers: sums and quotients in double and in long double, each
 * checked, every round, against the same work done in integers.
 *
 *   cfloat SEED [ROUNDS]
 *
 * Several copies with different seeds, run at once, are the test that
 * the kernel keeps each thread's floating-point registers its own: if a
 * switch let one copy's registers into another, that copy's running
 * sums would stop matching its integers, and it would say so. The sums
 * stay below 2^53, where a double holds every whole number exactly, so
 * a right answer is exactly right - there is no rounding to excuse a
 * wrong one. */

#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: cfloat SEED [ROUNDS]\n");
        return 2;
    }
    long seed = strtol(argv[1], 0, 10);
    long rounds = argc > 2 ? strtol(argv[2], 0, 10) : 30000000;
    long wrong = 0;
    long whole = 0;
    double sum = 0;
    long double long_sum = 0;
    for (long k = 1; k <= rounds; k++) {
        long term = (k * seed) % 1000;
        whole += term;
        sum += (double)term;
        long_sum += (long double)term;
        /* A division and back: the quotient is not whole, the product
         * rounds to the term again. */
        double third = (double)term / 7.0;
        if ((long)(third * 7.0 + 0.5) != term) {
            wrong++;
        }
        if ((long)sum != whole || (long)long_sum != whole) {
            wrong++;
            /* Start again from the right value, so one fault is
             * counted once. */
            sum = (double)whole;
            long_sum = (long double)whole;
        }
    }
    printf("cfloat %ld: %ld rounds, %ld wrong\n", seed, rounds, wrong);
    return wrong != 0;
}
