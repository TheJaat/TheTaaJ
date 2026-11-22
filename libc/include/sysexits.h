/* sysexits.h - exit codes with agreed meanings, from BSD. A program
 * that ends with one of these says what kind of failure it was: the
 * shell shows the number like any other exit code. */

#ifndef _SYSEXITS_H
#define _SYSEXITS_H

#define EX_OK 0            /* it worked */
#define EX_USAGE 64        /* the command was given wrongly */
#define EX_DATAERR 65      /* the input was wrong */
#define EX_NOINPUT 66      /* an input file was not there */
#define EX_NOUSER 67
#define EX_NOHOST 68
#define EX_UNAVAILABLE 69  /* a service it needs is not there */
#define EX_SOFTWARE 70     /* a bug in the program itself */
#define EX_OSERR 71        /* the system failed it */
#define EX_OSFILE 72
#define EX_CANTCREAT 73    /* an output file could not be made */
#define EX_IOERR 74        /* reading or writing failed */
#define EX_TEMPFAIL 75     /* try again later */
#define EX_PROTOCOL 76
#define EX_NOPERM 77       /* not allowed */
#define EX_CONFIG 78

#endif
