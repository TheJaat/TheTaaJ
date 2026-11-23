/* mmap.c - memory mappings as copies (sys/mman.h says what that means
 * and what it cannot do). */

#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define PAGE 4096

/* The mappings made, so that munmap knows a start from any other
 * pointer and frees only what mmap gave. */
static struct mapping {
    void *start;
    size_t length;
} *mappings;
static size_t mapping_count, mapping_room;

static int remember(void *start, size_t length)
{
    if (mapping_count == mapping_room) {
        size_t room = mapping_room ? mapping_room * 2 : 16;
        struct mapping *more = realloc(mappings, room * sizeof *more);
        if (more == NULL)
            return -1;
        mappings = more;
        mapping_room = room;
    }
    mappings[mapping_count].start = start;
    mappings[mapping_count].length = length;
    mapping_count++;
    return 0;
}

void *mmap(void *address, size_t length, int protection, int flags, int fd, off_t offset)
{
    (void)address; /* a hint only, without MAP_FIXED */
    if (length == 0 || (flags & MAP_FIXED) || offset < 0 || offset % PAGE != 0
        || !(flags & (MAP_SHARED | MAP_PRIVATE))) {
        errno = EINVAL;
        return MAP_FAILED;
    }
    if (protection & PROT_EXEC) {
        errno = EACCES;
        return MAP_FAILED;
    }
    if (!(flags & MAP_ANONYMOUS) && (flags & MAP_SHARED) && (protection & PROT_WRITE)) {
        errno = ENODEV;
        return MAP_FAILED;
    }
    size_t rounded = (length + PAGE - 1) / PAGE * PAGE;
    if (rounded < length) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    char *memory = aligned_alloc(PAGE, rounded);
    if (memory == NULL) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    memset(memory, 0, rounded);
    if (!(flags & MAP_ANONYMOUS)) {
        /* Read through the descriptor and put its place back, so that
         * mapping a file does not move where the program reads it. */
        off_t was = lseek(fd, 0, SEEK_CUR);
        if (was < 0 || lseek(fd, offset, SEEK_SET) < 0) {
            int why = errno;
            free(memory);
            errno = why == ESPIPE ? ENODEV : why;
            return MAP_FAILED;
        }
        size_t got = 0;
        while (got < length) {
            ssize_t n = read(fd, memory + got, length - got);
            if (n < 0) {
                int why = errno;
                lseek(fd, was, SEEK_SET);
                free(memory);
                errno = why;
                return MAP_FAILED;
            }
            if (n == 0)
                break; /* past the end of the file: zeros, as on Unix */
            got += (size_t)n;
        }
        lseek(fd, was, SEEK_SET);
    }
    if (remember(memory, rounded) != 0) {
        free(memory);
        errno = ENOMEM;
        return MAP_FAILED;
    }
    return memory;
}

int munmap(void *address, size_t length)
{
    if (length == 0) {
        errno = EINVAL;
        return -1;
    }
    for (size_t i = 0; i < mapping_count; i++) {
        if (mappings[i].start == address) {
            if (length < mappings[i].length)
                return 0; /* part of a mapping: it stays, whole (sys/mman.h) */
            free(address);
            mappings[i] = mappings[--mapping_count];
            return 0;
        }
    }
    /* Not a mapping's start: POSIX lets unmapping what is not mapped
     * succeed; but memory mmap never gave is not this one's to free. */
    return 0;
}

int mprotect(void *address, size_t length, int protection)
{
    (void)address;
    (void)length;
    if (protection & PROT_EXEC) {
        errno = EACCES;
        return -1;
    }
    return 0;
}

int msync(void *address, size_t length, int flags)
{
    (void)address;
    (void)length;
    (void)flags;
    return 0; /* nothing is shared with a file */
}

int madvise(void *address, size_t length, int advice)
{
    (void)address;
    (void)length;
    (void)advice;
    return 0;
}

int posix_madvise(void *address, size_t length, int advice)
{
    return madvise(address, length, advice);
}

/* __clear_cache(start, end): makes code just written to memory safe to
 * run - on AArch64 the instruction cache must be told. Normally the
 * compiler's runtime library has it; LLVM calls it after changing a
 * mapping's protection (sys::Memory::protectMappedMemory). On TheTaaJ a
 * program cannot make memory executable (PROT_EXEC is refused above),
 * so no code it writes is ever run, and there is nothing to make safe.
 * Only the kernel loads code, and it keeps the caches right itself. */
void __clear_cache(void *start, void *end)
{
    (void)start;
    (void)end;
}
