/* sys/mman.h - memory mappings.
 *
 * On Unix, mmap puts a file's pages straight into a program's memory,
 * or gives it fresh zeroed pages. TheTaaJ's kernel reserves address
 * space whose pages get zeroed memory when first touched (MEMORY_
 * RESERVE), and maps memory objects; the file service is a separate
 * program, so files are not mapped. So here:
 *
 *   MAP_ANONYMOUS      fresh zeroed memory, page-aligned, reserved: a
 *                      page takes memory only once touched, so mapping
 *                      1 GiB and using 1 MiB costs 1 MiB
 *   a file, MAP_PRIVATE
 *                      the file's bytes from `offset`, read into such
 *                      memory: a copy, which the program may change
 *                      without changing the file - what MAP_PRIVATE
 *                      means anyway. A change to the file afterwards is
 *                      not seen.
 *   a file, MAP_SHARED and PROT_WRITE
 *                      fails (ENODEV): changes could not reach the file
 *   PROT_EXEC          fails (EACCES): a program cannot make memory it
 *                      can run - the kernel maps nothing both writable
 *                      and executable, and code only from a program's
 *                      own file
 *   MAP_FIXED          fails (EINVAL): the place cannot be chosen
 *
 * munmap with a mapping's start and its whole length gives every page
 * of it back; asked for less, it leaves the mapping whole (memory is
 * given back a whole mapping at a time). Pages are always readable and
 * writable: PROT_READ alone, or PROT_NONE, is not enforced, and
 * mprotect refuses PROT_EXEC and otherwise changes nothing. Memory
 * touched past the program's limit (setrlimit's RLIMIT_AS) ends the
 * program, "out of memory". The numbers are Linux's. */

#ifndef _SYS_MMAN_H
#define _SYS_MMAN_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PROT_NONE 0
#define PROT_READ 1
#define PROT_WRITE 2
#define PROT_EXEC 4

#define MAP_SHARED 0x01
#define MAP_PRIVATE 0x02
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_ANON MAP_ANONYMOUS
#define MAP_NORESERVE 0x4000

#define MAP_FAILED ((void *)-1)

#define MS_ASYNC 1
#define MS_INVALIDATE 2
#define MS_SYNC 4

#define MADV_NORMAL 0
#define MADV_RANDOM 1
#define MADV_SEQUENTIAL 2
#define MADV_WILLNEED 3
#define MADV_DONTNEED 4
#define POSIX_MADV_NORMAL MADV_NORMAL
#define POSIX_MADV_RANDOM MADV_RANDOM
#define POSIX_MADV_SEQUENTIAL MADV_SEQUENTIAL
#define POSIX_MADV_WILLNEED MADV_WILLNEED
#define POSIX_MADV_DONTNEED MADV_DONTNEED

void *mmap(void *address, size_t length, int protection, int flags, int fd, off_t offset);
int munmap(void *address, size_t length);
int mprotect(void *address, size_t length, int protection);
int msync(void *address, size_t length, int flags);
/* Advice: taken, and not acted on. */
int madvise(void *address, size_t length, int advice);
int posix_madvise(void *address, size_t length, int advice);

#ifdef __cplusplus
}
#endif

#endif
