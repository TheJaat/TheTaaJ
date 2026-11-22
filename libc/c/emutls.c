/* emutls.c - thread-local variables, for a program with one thread.
 *
 * A `thread_local` variable in C++ (C's `_Thread_local`) has a copy for
 * each thread. Usually the compiler finds the copy through a register
 * the kernel points at the thread's block of them (FS on the PC,
 * TPIDR_EL0 on AArch64), set up from the program's .tdata and .tbss
 * sections. TheTaaJ does not set that register up for programs yet.
 *
 * So TheTaaJ's C++ is compiled with -femulated-tls, which asks for the
 * other way, the one compilers keep for systems like this: for each
 * such variable the compiler makes a small record - its size, its
 * alignment, a slot, and the bytes it starts with - and every use of
 * the variable becomes a call to __emutls_get_address(&record), which
 * returns where this thread's copy is.
 *
 * With one thread there is one copy, made the first time it is asked
 * for, and its address kept in the record's slot. (With threads, the
 * slot would hold an index into a table per thread instead.) */

#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* The record, as the compiler lays it out. */
struct __emutls_control {
    size_t size;      /* of the variable */
    size_t align;     /* its alignment */
    void *object;     /* the slot: here, the copy's address once made */
    const void *value; /* the bytes it starts with, or null for zeros */
};

void *__emutls_get_address(struct __emutls_control *control);

void *__emutls_get_address(struct __emutls_control *control)
{
    if (control->object)
        return control->object;
    size_t align = control->align < sizeof(void *) ? sizeof(void *) : control->align;
    /* aligned_alloc wants a size that is a whole number of alignments. */
    size_t size = (control->size + align - 1) / align * align;
    void *copy = aligned_alloc(align, size ? size : align);
    if (copy == NULL)
        abort(); /* the variable must be somewhere, and nothing else can be said */
    if (control->value)
        memcpy(copy, control->value, control->size);
    else
        memset(copy, 0, control->size);
    control->object = copy;
    return copy;
}
