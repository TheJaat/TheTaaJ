#ifndef __LIBRT_SYSCALL_H__
#define __LIBRT_SYSCALL_H__

#include <os/syscalls.h>

/* The user-space runtime.
 *
 * A user program has no libc and no runtime beyond this. It is compiled
 * to a relocatable object and linked into the module with `ld -r`, so
 * these are ordinary internal calls by the time the kernel sees the
 * module - a user module must have NO undefined symbols, because ring 3
 * cannot call a ring-0 address and the loader refuses any module that
 * tries. */

typedef unsigned int size_t_u;

/* -- raw trap ------------------------------------------------------- */
int SysCall(int Number, unsigned A, unsigned B, unsigned C);

/* -- process -------------------------------------------------------- */
void         SysExit(int Code);
void         SysSleep(unsigned Ms);
void         SysYield(void);
unsigned     SysGetMs(void);
int          SysGetTid(void);
int          SysGetPid(void);

/* -- output --------------------------------------------------------- */
int          SysWrite(const char *Text, unsigned Length);
void         SysPrint(const char *Text);              /* NUL-terminated */
void         SysPrintNumber(unsigned Value);
void         SysPrintLine(const char *Text);

/* -- handles -------------------------------------------------------- */
int          SysHandleClose(int Handle);

/* -- ipc ------------------------------------------------------------ */
int          SysPipeCreate(unsigned Size, unsigned Flags);
int          SysPipeWrite(int Handle, const void *Buffer, unsigned Length);
int          SysPipeRead(int Handle, void *Buffer, unsigned Length);
int          SysPipeAvailable(int Handle);

/* -- naming --------------------------------------------------------- */
int          SysRegisterName(const char *Name, int PipeHandle);
int          SysLookupName(const char *Name);

/* -- hardware, servers only ----------------------------------------- */
int          SysIrqRegister(int Line);
int          SysIrqWait(int Handle);
int          SysIrqAck(int Handle);
int          SysIoRequest(unsigned Port, unsigned Count);

/* Port access, once SysIoRequest has granted the range. These are plain
 * in/out instructions - the cpu checks the I/O permission bitmap, so
 * there is no syscall and no cost per access. Executing one on a port
 * that was not granted is a general protection fault. */
unsigned char SysInB(unsigned short Port);
void          SysOutB(unsigned short Port, unsigned char Value);

/* -- minimal string helpers ----------------------------------------- */
unsigned     SysStringLength(const char *Text);
void         SysMemSet(void *Destination, int Value, unsigned Length);
void         SysMemCopy(void *Destination, const void *Source, unsigned Length);
int          SysStringCompare(const char *A, const char *B);

#endif /* __LIBRT_SYSCALL_H__ */
