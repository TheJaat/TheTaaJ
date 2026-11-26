/* mmap.c - memory mappings (sys/mman.h says what they are here and
 * what they cannot do).
 *
 * Every mapping is reserved memory from the kernel (MEMORY_RESERVE,
 * through __taaj_reserve in memory.rs): address space whose pages get
 * zeroed memory only when first touched. So a program may map 1 GiB
 * and pay for the pages it uses - what allocators and compilers that
 * mmap big arenas count on - and munmap gives every page back. A file
 * is read into such memory: a copy - for MAP_SHARED with PROT_WRITE,
 * one written back to the file on msync, munmap and exit. mprotect, madvise's
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
    /* A shared, writable mapping of a file: a descriptor of its own for
     * it (dup), where in the file it starts, and how many of its bytes
     * came from the file - what is written back. -1 for any other. */
    int fd;
    off_t offset;
    size_t file_bytes;
} *mappings;
static size_t mapping_count, mapping_room;

/* Writes `length` bytes of shared mapping `m` from `from` back to its
 * file - only what came from the file: a mapping never makes the file
 * longer, as on Unix. 0, or -1 with errno. The caller holds the lock. */
static int write_back(const struct mapping *m, char *from, size_t length)
{
    if (m->fd < 0)
        return 0;
    char *start = m->start, *file_end = start + m->file_bytes;
    char *end = from + length < file_end ? from + length : file_end;
    while (from < end) {
        ssize_t n = pwrite(m->fd, from, (size_t)(end - from), m->offset + (from - start));
        if (n <= 0)
            return -1;
        from += n;
    }
    return 0;
}

/* At exit, every shared mapping still there goes back to its file, as
 * Unix's do when a program ends. */
static void write_all_back(void)
{
    pthread_mutex_lock(&lock);
    for (size_t i = 0; i < mapping_count; i++)
        write_back(&mappings[i], mappings[i].start, mappings[i].length);
    pthread_mutex_unlock(&lock);
}

static int remember(void *start, size_t length, int fd, off_t offset, size_t file_bytes)
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
        mappings[mapping_count].fd = fd;
        mappings[mapping_count].offset = offset;
        mappings[mapping_count].file_bytes = file_bytes;
        mapping_count++;
    }
    pthread_mutex_unlock(&lock);
    return result;
}

static void register_write_all_back(void)
{
    atexit(write_all_back);
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
    size_t got = 0;
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
    /* Shared and writable: what the program writes goes back to the
     * file (msync, munmap, exit), through a descriptor of the mapping's
     * own - the program may close its own. */
    int shared = -1;
    if (!(flags & MAP_ANONYMOUS) && (flags & MAP_SHARED) && (protection & PROT_WRITE)) {
        shared = dup(fd);
        if (shared < 0) {
            int why = errno;
            __taaj_unmap(memory);
            errno = why;
            return MAP_FAILED;
        }
        static pthread_once_t once = PTHREAD_ONCE_INIT;
        pthread_once(&once, register_write_all_back);
    }
    if (remember(memory, rounded, shared, offset, got) != 0) {
        if (shared >= 0)
            close(shared);
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
            write_back(&mappings[i], start, mappings[i].length);
            if (mappings[i].fd >= 0)
                close(mappings[i].fd);
            __taaj_unmap(address);
            mappings[i] = mappings[--mapping_count];
        } else {
            /* Part of one: written back if it is shared, then its memory
             * goes back, and it is put out of reach, as an unmapped page
             * would be; the address space stays reserved until the whole
             * is unmapped (sys/mman.h). */
            size_t part = rounded < (size_t)(end - from) ? rounded : (size_t)(end - from);
            write_back(&mappings[i], from, part);
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
    (void)flags; /* MS_ASYNC is done at once too; MS_INVALIDATE has nothing to drop */
    if ((size_t)address % PAGE != 0) {
        errno = EINVAL;
        return -1;
    }
    char *from = address;
    int result = 0;
    pthread_mutex_lock(&lock);
    size_t i = 0;
    for (; i < mapping_count; i++) {
        char *start = mappings[i].start, *end = start + mappings[i].length;
        if (from >= start && from < end) {
            size_t part = length < (size_t)(end - from) ? length : (size_t)(end - from);
            if (write_back(&mappings[i], from, part) != 0)
                result = -1; /* errno from pwrite */
            break;
        }
    }
    if (i == mapping_count) {
        errno = ENOMEM; /* not mapped */
        result = -1;
    }
    pthread_mutex_unlock(&lock);
    return result;
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
