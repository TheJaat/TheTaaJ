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

/* -- hardware -------------------------------------------------------- */

int SysIrqRegister(int Line)
{
    return SysCall(SYS_IRQ_REGISTER, (unsigned)Line, 0, 0);
}

int SysIrqWait(int Handle)
{
    return SysCall(SYS_IRQ_WAIT, (unsigned)Handle, 0, 0);
}

int SysIrqAck(int Handle)
{
    return SysCall(SYS_IRQ_ACK, (unsigned)Handle, 0, 0);
}

int SysIoRequest(unsigned Port, unsigned Count)
{
    return SysCall(SYS_IO_REQUEST, Port, Count, 0);
}

unsigned int SysIoMap(unsigned Physical, unsigned Length)
{
    return (unsigned)SysCall(SYS_IO_MAP, Physical, Length, 0);
}

int SysSpawn(const char *ModuleName)
{
    return SysCall(SYS_SPAWN, (unsigned)ModuleName, 0, 0);
}

int SysProcessAlive(int Pid)
{
    return SysCall(SYS_PROCESS_ALIVE, (unsigned)Pid, 0, 0);
}

unsigned int SysMmioRead32(unsigned int Address)
{
    return *(volatile unsigned int*)Address;
}

void SysMmioWrite32(unsigned int Address, unsigned int Value)
{
    *(volatile unsigned int*)Address = Value;
}

unsigned char SysInB(unsigned short Port)
{
    unsigned char Value;
    __asm__ volatile ("inb %1, %0" : "=a"(Value) : "Nd"(Port));
    return Value;
}

void SysOutB(unsigned short Port, unsigned char Value)
{
    __asm__ volatile ("outb %0, %1" :: "a"(Value), "Nd"(Port));
}

unsigned int SysInL(unsigned short Port)
{
    unsigned int Value;
    __asm__ volatile ("inl %1, %0" : "=a"(Value) : "Nd"(Port));
    return Value;
}

void SysOutL(unsigned short Port, unsigned int Value)
{
    __asm__ volatile ("outl %0, %1" :: "a"(Value), "Nd"(Port));
}

/* -- synchronous ipc ------------------------------------------------- */

int SysEndpointCreate(void)
{
    return SysCall(SYS_ENDPOINT_CREATE, 0, 0, 0);
}

int SysCall2(int Endpoint, unsigned Opcode,
             const void *Send, unsigned SendLength,
             void *Recv, unsigned RecvLength)
{
    SysCallArgs_t Args;

    Args.Endpoint   = Endpoint;
    Args.Opcode     = Opcode;
    Args.SendLength = SendLength;
    Args.RecvLength = RecvLength;
    Args.SendBuffer = (void*)Send;
    Args.RecvBuffer = Recv;

    return SysCall(SYS_CALL, (unsigned)&Args, 0, 0);
}

int SysRecv(int Endpoint, void *Buffer, unsigned Length,
            unsigned *Opcode, unsigned *Badge)
{
    SysRecvArgs_t Args;
    int Result;

    Args.Endpoint = Endpoint;
    Args.Length   = Length;
    Args.Buffer   = Buffer;
    Args.Opcode   = 0;
    Args.Badge    = 0;

    Result = SysCall(SYS_RECV, (unsigned)&Args, 0, 0);

    if (Result >= 0) {
        if (Opcode != 0) { *Opcode = Args.Opcode; }
        if (Badge  != 0) { *Badge  = Args.Badge;  }
    }
    return Result;
}

int SysReply(const void *Buffer, unsigned Length)
{
    SysReplyArgs_t Args;

    Args.Length = Length;
    Args.Buffer = (void*)Buffer;

    return SysCall(SYS_REPLY, (unsigned)&Args, 0, 0);
}

int SysCapGrant(int Process, int Handle, unsigned Badge)
{
    SysGrantArgs_t Args;

    Args.Process = Process;
    Args.Handle  = Handle;
    Args.Badge   = Badge;

    return SysCall(SYS_CAP_GRANT, (unsigned)&Args, 0, 0);
}

int SysRegisterEndpoint(const char *Name, int Endpoint, int Shm)
{
    SysServiceArgs_t Args;
    Args.Name = Name; Args.Endpoint = Endpoint; Args.Shm = Shm;
    return SysCall(SYS_REGISTER_ENDPOINT, (unsigned)&Args, 0, 0);
}

int SysLookupEndpoint(const char *Name, int *Endpoint, int *Shm)
{
    SysServiceArgs_t Args;
    int Result;

    Args.Name = Name; Args.Endpoint = -1; Args.Shm = -1;
    Result = SysCall(SYS_LOOKUP_ENDPOINT, (unsigned)&Args, 0, 0);

    if (Result == SYSCALL_OK) {
        if (Endpoint != 0) { *Endpoint = Args.Endpoint; }
        if (Shm != 0)      { *Shm = Args.Shm; }
    }
    return Result;
}

/* -- shared memory ---------------------------------------------------- */

int SysShmCreate(unsigned Length)
{
    return SysCall(SYS_SHM_CREATE, Length, 0, 0);
}

unsigned SysShmMap(int Handle)
{
    return (unsigned)SysCall(SYS_SHM_MAP, (unsigned)Handle, 0, 0);
}

int SysShmSize(int Handle)
{
    return SysCall(SYS_SHM_SIZE, (unsigned)Handle, 0, 0);
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
