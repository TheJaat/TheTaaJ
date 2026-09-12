#ifndef __SHARED_SYSCALLS_H__
#define __SHARED_SYSCALLS_H__

/* Shared between the kernel and every user program.
 *
 * This header is the ABI. User binaries are compiled against these
 * numbers and the kernel dispatches on them, so entries may be appended
 * but never reordered or renumbered - a stale binary would then call the
 * wrong thing, which is far worse than failing to link.
 *
 * Kept outside both kernel/ and librt/ so neither can drift from the
 * other: there is exactly one copy. */

#define SYSCALL_VECTOR              128

/* -- process and thread ------------------------------------------- */
#define SYS_EXIT                    0   /* ebx = code                  */
#define SYS_WRITE                   1   /* ebx = buf, ecx = len        */
#define SYS_SLEEP                   2   /* ebx = ms                    */
#define SYS_GETMS                   3
#define SYS_GETTID                  4
#define SYS_GETPID                  5
#define SYS_YIELD                   6

/* -- handles ------------------------------------------------------- */
#define SYS_HANDLE_CLOSE            7   /* ebx = handle                */

/* -- ipc ----------------------------------------------------------- */
#define SYS_PIPE_CREATE             8   /* ebx = size, ecx = flags     */
#define SYS_PIPE_WRITE              9   /* ebx = h, ecx = buf, edx = len */
#define SYS_PIPE_READ              10   /* ebx = h, ecx = buf, edx = len */
#define SYS_PIPE_AVAILABLE         11   /* ebx = handle                */

/* -- naming -------------------------------------------------------- */
#define SYS_REGISTER_NAME          12   /* ebx = name, ecx = pipe      */
#define SYS_LOOKUP_NAME            13   /* ebx = name -> handle        */

#define SYS_MAX                    14

/* Errors. Syscalls return a negative value on failure so a caller can
 * distinguish "0 bytes" from "failed". */
#define SYSCALL_OK                  0
#define SYSCALL_ERROR              (-1)
#define SYSCALL_BADHANDLE          (-2)
#define SYSCALL_BADPOINTER         (-3)
#define SYSCALL_DENIED             (-4)
#define SYSCALL_NOTFOUND           (-5)
#define SYSCALL_WOULDBLOCK         (-6)

/* Pipe flags, mirroring the kernel's. */
#define PIPE_FLAG_NOBLOCK_READ      0x1
#define PIPE_FLAG_NOBLOCK_WRITE     0x2

#define NAME_MAX_LENGTH             32

#endif /* __SHARED_SYSCALLS_H__ */
