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

/* -- rpc ----------------------------------------------------------- */
#define SYS_SET_REPLY              14   /* ebx = pipe handle           */
#define SYS_OPEN_REPLY             15   /* ebx = pid -> handle         */

/* -- hardware, servers only ---------------------------------------- */
#define SYS_IRQ_REGISTER           16   /* ebx = line -> handle        */
#define SYS_IRQ_WAIT               17   /* ebx = handle, blocks        */
#define SYS_IRQ_ACK                18   /* ebx = handle                */
#define SYS_IO_REQUEST             19   /* ebx = port, ecx = count     */

/* -- device memory and process control, servers only ---------------- */
#define SYS_IO_MAP                 20   /* ebx = phys, ecx = size -> va */
#define SYS_SPAWN                  21   /* ebx = module name -> pid     */
#define SYS_PROCESS_ALIVE          22   /* ebx = pid -> 1 or 0          */

/* -- synchronous ipc ------------------------------------------------ */
#define SYS_ENDPOINT_CREATE        23   /* -> handle                    */
#define SYS_CALL                   24   /* ebx = args block             */
#define SYS_RECV                   25   /* ebx = args block, blocks     */
#define SYS_REPLY                  26   /* ebx = args block             */

/* -- capabilities --------------------------------------------------- */
#define SYS_CAP_GRANT              27   /* ebx = args block             */

/* -- shared memory -------------------------------------------------- */
#define SYS_SHM_CREATE             28   /* ebx = size -> handle         */
#define SYS_SHM_MAP                29   /* ebx = handle -> address      */
#define SYS_SHM_SIZE               30   /* ebx = handle -> bytes        */

#define SYS_REGISTER_ENDPOINT      31   /* ebx = args block             */
#define SYS_LOOKUP_ENDPOINT        32   /* ebx = args block             */

#define SYS_MAX                    33

/* A call carries more than three arguments, so they are passed as a
 * block rather than in registers. The kernel validates the block like
 * any other user pointer before reading it. */
typedef struct _SysCallArgs {
    int          Endpoint;      /* capability being invoked        */
    unsigned int Opcode;
    unsigned int SendLength;
    unsigned int RecvLength;
    void        *SendBuffer;
    void        *RecvBuffer;
} SysCallArgs_t;

typedef struct _SysRecvArgs {
    int          Endpoint;
    unsigned int Length;        /* buffer size in, message size out */
    void        *Buffer;
    unsigned int Opcode;        /* out */
    unsigned int Badge;         /* out - who called, kernel-stamped */
} SysRecvArgs_t;

typedef struct _SysReplyArgs {
    unsigned int Length;
    void        *Buffer;
} SysReplyArgs_t;

typedef struct _SysGrantArgs {
    int          Process;       /* target process id      */
    int          Handle;        /* capability to hand over */
    unsigned int Badge;         /* stamped on the copy     */
} SysGrantArgs_t;

typedef struct _SysServiceArgs {
    const char  *Name;
    int          Endpoint;      /* in on register, out on lookup */
    int          Shm;           /* in on register, out on lookup; -1 none */
} SysServiceArgs_t;

/* The largest message a single call may carry. Small on purpose: this
 * is a control path, and bulk data belongs in shared memory. */
#define IPC_MESSAGE_MAX             256

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
#define PIPE_FLAG_ATOMIC            0x4

#define NAME_MAX_LENGTH             32

#endif /* __SHARED_SYSCALLS_H__ */
