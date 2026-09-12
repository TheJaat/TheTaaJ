/* The user-space runtime. Compiled into every user module. */

#include <os/syscall.h>

/* SysCall
 * eax = number, ebx/ecx/edx = arguments, eax = result.
 *
 * "memory" in the clobber list is not decoration: without it the
 * compiler may keep a buffer in a register across the trap, and the
 * kernel then reads stale bytes out of it. Every syscall passing a
 * pointer depends on this. */
int SysCall(int Number, unsigned A, unsigned B, unsigned C)
{
    int Result;

    __asm__ volatile ("int $0x80"
        : "=a"(Result)
        : "a"(Number), "b"(A), "c"(B), "d"(C)
        : "memory");

    return Result;
}

/* -- process -------------------------------------------------------- */

void SysExit(int Code)
{
    SysCall(SYS_EXIT, (unsigned)Code, 0, 0);
    /* Not reached: exit does not return. If it somehow did, spinning is
     * better than falling off the end of a stack with nothing on it. */
    for (;;) { }
}

void     SysSleep(unsigned Ms)  { SysCall(SYS_SLEEP, Ms, 0, 0); }
void     SysYield(void)         { SysCall(SYS_YIELD, 0, 0, 0); }
unsigned SysGetMs(void)         { return (unsigned)SysCall(SYS_GETMS, 0, 0, 0); }
int      SysGetTid(void)        { return SysCall(SYS_GETTID, 0, 0, 0); }
int      SysGetPid(void)        { return SysCall(SYS_GETPID, 0, 0, 0); }

/* -- output --------------------------------------------------------- */

int SysWrite(const char *Text, unsigned Length)
{
    return SysCall(SYS_WRITE, (unsigned)Text, Length, 0);
}

void SysPrint(const char *Text)
{
    SysWrite(Text, SysStringLength(Text));
}

void SysPrintLine(const char *Text)
{
    SysPrint(Text);
    SysPrint("\n");
}

void SysPrintNumber(unsigned Value)
{
    char Digits[16];
    char Out[17];
    int i = 0, j = 0;

    if (Value == 0) {
        SysPrint("0");
        return;
    }
    while (Value > 0) {
        Digits[i++] = (char)('0' + (Value % 10));
        Value /= 10;
    }
    while (i > 0) {
        Out[j++] = Digits[--i];
    }
    Out[j] = '\0';
    SysPrint(Out);
}

/* -- handles and ipc ------------------------------------------------ */

int SysHandleClose(int Handle)
{
    return SysCall(SYS_HANDLE_CLOSE, (unsigned)Handle, 0, 0);
}

int SysPipeCreate(unsigned Size, unsigned Flags)
{
    return SysCall(SYS_PIPE_CREATE, Size, Flags, 0);
}

int SysPipeWrite(int Handle, const void *Buffer, unsigned Length)
{
    return SysCall(SYS_PIPE_WRITE, (unsigned)Handle, (unsigned)Buffer, Length);
}

int SysPipeRead(int Handle, void *Buffer, unsigned Length)
{
    return SysCall(SYS_PIPE_READ, (unsigned)Handle, (unsigned)Buffer, Length);
}

int SysPipeAvailable(int Handle)
{
    return SysCall(SYS_PIPE_AVAILABLE, (unsigned)Handle, 0, 0);
}

int SysRegisterName(const char *Name, int PipeHandle)
{
    return SysCall(SYS_REGISTER_NAME, (unsigned)Name, (unsigned)PipeHandle, 0);
}

int SysLookupName(const char *Name)
{
    return SysCall(SYS_LOOKUP_NAME, (unsigned)Name, 0, 0);
}

/* -- string helpers -------------------------------------------------- */

unsigned SysStringLength(const char *Text)
{
    unsigned Length = 0;

    if (Text == 0) {
        return 0;
    }
    while (Text[Length] != '\0') {
        Length++;
    }
    return Length;
}

void SysMemSet(void *Destination, int Value, unsigned Length)
{
    unsigned char *p = (unsigned char*)Destination;
    unsigned i;

    for (i = 0; i < Length; i++) {
        p[i] = (unsigned char)Value;
    }
}

void SysMemCopy(void *Destination, const void *Source, unsigned Length)
{
    unsigned char *d = (unsigned char*)Destination;
    const unsigned char *s = (const unsigned char*)Source;
    unsigned i;

    for (i = 0; i < Length; i++) {
        d[i] = s[i];
    }
}

int SysStringCompare(const char *A, const char *B)
{
    const unsigned char *p = (const unsigned char*)A;
    const unsigned char *q = (const unsigned char*)B;

    while (*p != '\0' && *p == *q) {
        p++;
        q++;
    }
    return (int)*p - (int)*q;
}
