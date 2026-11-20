/* stdlib.h - the general-purpose part of the C library. */
#ifndef _STDLIB_H
#define _STDLIB_H

#include <stddef.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

/* Ends the program. Returning from main does the same. */
_Noreturn void exit(int code);

/* Memory, asked for while the program runs. Each returns NULL if there
 * is none to give.
 *
 *   malloc    size bytes, holding whatever was there
 *   calloc    count * size bytes, all zero
 *   realloc   the same bytes in a block of a new size - which may be
 *             somewhere else: use only the address returned
 *   free      gives a block back; free(NULL) does nothing */
void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *block, size_t size);
void free(void *block);

#endif
