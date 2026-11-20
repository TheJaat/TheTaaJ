/* stdlib.h - the general-purpose part of the C library. */
#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

/* Ends the program. Returning from main does the same. */
_Noreturn void exit(int code);

#endif
