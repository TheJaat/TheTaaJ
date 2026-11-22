/* atexit.c - what runs before main, and what runs after it.
 *
 * Before: the constructors. A C++ object defined outside any function
 * (`std::string greeting = "hello";`) must be made before main starts,
 * and a C function marked __attribute__((constructor)) runs then too.
 * The compiler puts a pointer to each such piece of code in a section
 * called .init_array; the linker gathers them all into one array, and
 * marks where it begins and ends with two symbols (user/cxx.ld, or
 * TCC's own linker). Here they are called, in order.
 *
 * After: the handlers. exit - or returning from main - calls, last
 * registered first:
 *
 *   atexit(f)                 what C programs ask for
 *   __cxa_atexit(f, p, dso)   what C++ asks for, for every object
 *                             outside a function that has a destructor:
 *                             the compiler makes the call right after
 *                             making the object, so objects are
 *                             destroyed in the opposite order to the one
 *                             they were made in
 *
 * and then the .fini_array functions (destructor-marked C functions),
 * from last to first. _start (libc/src/lib.rs) calls the first half,
 * exit the second.
 *
 * As many handlers as memory allows: 32 at a time in a block, and
 * another block from malloc when one is full. */

#include <stddef.h>
#include <stdlib.h>

/* Where the linker put the arrays. Weak: a program linked without the
 * symbols (by some other linker script) gets null for both ends of an
 * array, which is then empty. */
extern void (*__init_array_start[])(void) __attribute__((weak));
extern void (*__init_array_end[])(void) __attribute__((weak));
extern void (*__fini_array_start[])(void) __attribute__((weak));
extern void (*__fini_array_end[])(void) __attribute__((weak));

/* Which shared object a C++ handler belongs to, so that unloading one
 * could run just its handlers. A TheTaaJ program is one object, all
 * static, so this only has to exist: the compiler passes its address. */
void *__dso_handle = &__dso_handle;

#define BLOCK 32

struct handler {
    void (*function)(void *);
    void *argument;
    int c_style; /* atexit's: called with no argument */
};

struct block {
    struct block *older;
    int used;
    struct handler handlers[BLOCK];
};

/* The newest block. The first is not from malloc, so the first 32 can
 * never fail - C promises at least 32 atexit calls will work. */
static struct block first;
static struct block *newest = &first;

static int add(void (*function)(void *), void *argument, int c_style)
{
    if (newest->used == BLOCK) {
        struct block *more = malloc(sizeof *more);
        if (more == NULL)
            return -1;
        more->older = newest;
        more->used = 0;
        newest = more;
    }
    struct handler *h = &newest->handlers[newest->used++];
    h->function = function;
    h->argument = argument;
    h->c_style = c_style;
    return 0;
}

int __cxa_atexit(void (*function)(void *), void *argument, void *dso)
{
    (void)dso;
    return add(function, argument, 0);
}

int atexit(void (*function)(void))
{
    return add((void (*)(void *))function, NULL, 1);
}

void __taaj_run_constructors(void)
{
    if (__init_array_start == NULL)
        return;
    for (void (**f)(void) = __init_array_start; f < __init_array_end; f++)
        (*f)();
}

void __taaj_run_destructors(void)
{
    /* Newest first. Each handler is taken off before it is called: a
     * handler may itself call atexit (C says the new one then runs
     * too, next), or call exit, which must not run it again. */
    for (;;) {
        while (newest->used == 0 && newest != &first) {
            struct block *done = newest;
            newest = newest->older;
            free(done);
        }
        if (newest->used == 0)
            break;
        struct handler h = newest->handlers[--newest->used];
        if (h.c_style)
            ((void (*)(void))h.function)();
        else
            h.function(h.argument);
    }
    if (__fini_array_start == NULL)
        return;
    for (void (**f)(void) = __fini_array_end; f > __fini_array_start;)
        (*--f)();
}
