/* mmap.c - memory mappings (sys/mman.h says what they are here and
 * what they cannot do).
 *
 * Every mapping is reserved memory from the kernel (MEMORY_RESERVE,
 * through __taaj_reserve in memory.rs): address space whose pages get
 * zeroed memory only when first touched. So a program may map 1 GiB
 * and pay for the pages it uses - what allocators and compilers that
 * mmap big arenas count on - and munmap gives every page back. A file
 * is read into such memory: a private copy. mprotect, madvise's
 * MADV_DONTNEED and munmap of part of a mapping are the kernel's
 * MEMORY_PROTECT and MEMORY_DISCARD on that memory. */

#include <errno.h>
#include <pthread.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define PAGE 4096

void *__taaj_reserve(size_t size);
int __taaj_unmap(void *start);
int __taaj_protect(void *start, size_t length, int access);
int __taaj_discard(void *start, size_t length);

/* MEMORY_PROTECT's access for a protection: PROT_WRITE means reading
 * too, as on every CPU TheTaaJ runs on. */
static int access_for(int protection)
{
    if (protection & PROT_WRITE)
        return 2;
    return (protection & PROT_READ) ? 1 : 0;
}

/* The mappings made, so that munmap knows a start from any other
 * pointer and how long each is. Threads may map and unmap at once:
 * the list is theirs one at a time. */
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static struct mapping {
    void *start;
    size_t length;
} *mappings;
static size_t mapping_count, mapping_room;

static int remember(void *start, size_t length)
{
    pthread_mutex_lock(&lock);
    int result = 0;
    if (mapping_count == mapping_room) {
        size_t room = mapping_room ? mapping_room * 2 : 16;
        struct mapping *more = realloc(mappings, room * sizeof *more);
        if (more == NULL)
            result = -1;
        else {
            mappings = more;
            mapping_room = room;
        }
    }
    if (result == 0) {
        mappings[mapping_count].start = start;
        mappings[mapping_count].length = length;
        mapping_count++;
    }
    pthread_mutex_unlock(&lock);
    return result;
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
    char *memory = __taaj_reserve(rounded);
    if (memory == NULL) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    if (!(flags & MAP_ANONYMOUS)) {
        /* Read through the descriptor and put its place back, so that
         * mapping a file does not move where the program reads it. The
         * pages read into are touched, and so taken; the rest of the
         * mapping - past the end of the file - stays untouched zeros. */
        off_t was = lseek(fd, 0, SEEK_CUR);
        if (was < 0 || lseek(fd, offset, SEEK_SET) < 0) {
            int why = errno;
            __taaj_unmap(memory);
            errno = why == ESPIPE ? ENODEV : why;
            return MAP_FAILED;
        }
        size_t got = 0;
        while (got < length) {
            ssize_t n = read(fd, memory + got, length - got);
            if (n < 0) {
                int why = errno;
                lseek(fd, was, SEEK_SET);
                __taaj_unmap(memory);
                errno = why;
                return MAP_FAILED;
            }
            if (n == 0)
                break; /* past the end of the file: zeros, as on Unix */
            got += (size_t)n;
        }
        lseek(fd, was, SEEK_SET);
    }
    /* Read-only or out of reach, once filled: the file was read in
     * while the pages could still be written. */
    if (access_for(protection) != 2 && __taaj_protect(memory, rounded, access_for(protection)) != 0) {
        int why = errno;
        __taaj_unmap(memory);
        errno = why;
        return MAP_FAILED;
    }
    if (remember(memory, rounded) != 0) {
        __taaj_unmap(memory);
        errno = ENOMEM;
        return MAP_FAILED;
    }
    return memory;
}

int munmap(void *address, size_t length)
{
    if (length == 0 || (size_t)address % PAGE != 0) {
        errno = EINVAL;
        return -1;
    }
    char *from = address;
    size_t rounded = (length + PAGE - 1) / PAGE * PAGE;
    pthread_mutex_lock(&lock);
    for (size_t i = 0; i < mapping_count; i++) {
        char *start = mappings[i].start, *end = start + mappings[i].length;
        if (from < start || from >= end)
            continue;
        if (from == start && rounded >= mappings[i].length) {
            __taaj_unmap(address);
            mappings[i] = mappings[--mapping_count];
        } else {
            /* Part of one: its memory goes back, and it is put out of
             * reach, as an unmapped page would be; the address space
             * stays reserved until the whole is unmapped (sys/mman.h). */
            size_t part = rounded < (size_t)(end - from) ? rounded : (size_t)(end - from);
            __taaj_discard(from, part);
            __taaj_protect(from, part, 0);
        }
        break;
    }
    pthread_mutex_unlock(&lock);
    /* Not a mapping's start: POSIX lets unmapping what is not mapped
     * succeed; but memory mmap never gave is not this one's to free. */
    return 0;
}

int mprotect(void *address, size_t length, int protection)
{
    if (protection & PROT_EXEC) {
        errno = EACCES;
        return -1;
    }
    if ((size_t)address % PAGE != 0) {
        errno = EINVAL;
        return -1;
    }
    if (length == 0)
        return 0;
    size_t rounded = (length + PAGE - 1) / PAGE * PAGE;
    if (rounded < length) {
        errno = ENOMEM;
        return -1;
    }
    /* The kernel checks the rest: whole pages of reserved memory, all
     * in one piece - what mmap gives, and what malloc and the thread
     * stacks are made of. */
    return __taaj_protect(address, rounded, access_for(protection));
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
    if ((size_t)address % PAGE != 0) {
        errno = EINVAL;
        return -1;
    }
    if (advice != MADV_DONTNEED || length == 0)
        return 0; /* the others are hints, and there is nothing to tune */
    size_t rounded = (length + PAGE - 1) / PAGE * PAGE;
    return __taaj_discard(address, rounded);
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
