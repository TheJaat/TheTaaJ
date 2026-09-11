/* user - a ring 3 module for TheTaaJ.
 *
 * Runs at privilege level 3. It cannot call kernel functions: a call to
 * a ring-0 address from here is a protection fault, so the loader
 * refuses any module that references a kernel symbol. Everything goes
 * through int 0x80.
 *
 * Note there is no libc and no runtime here. ModuleMain is entered
 * directly by iret; it must not return, because there is nothing to
 * return to - the stack below it is zero. Call Exit instead. */

#define SYS_EXIT    0
#define SYS_WRITE   1
#define SYS_SLEEP   2
#define SYS_GETMS   3
#define SYS_GETTID  4

/* eax = call number, ebx/ecx/edx = arguments, eax = result.
 * "memory" in the clobber list matters: without it the compiler is free
 * to keep a buffer in a register across the trap, and the kernel would
 * read stale bytes out of it. */
static int Syscall(int number, unsigned a, unsigned b, unsigned c)
{
    int result;
    __asm__ volatile ("int $0x80"
        : "=a"(result)
        : "a"(number), "b"(a), "c"(b), "d"(c)
        : "memory");
    return result;
}

static unsigned StringLength(const char *s)
{
    unsigned n = 0;
    while (s[n] != '\0') {
        n++;
    }
    return n;
}

static void Write(const char *s)
{
    Syscall(SYS_WRITE, (unsigned)s, StringLength(s), 0);
}

static void WriteNumber(unsigned value)
{
    char buffer[16];
    int i = 0, j = 0;
    char out[17];

    if (value == 0) {
        Write("0");
        return;
    }
    while (value > 0) {
        buffer[i++] = (char)('0' + (value % 10));
        value /= 10;
    }
    while (i > 0) {
        out[j++] = buffer[--i];
    }
    out[j] = '\0';
    Write(out);
}

static void Exit(int code)
{
    Syscall(SYS_EXIT, (unsigned)code, 0, 0);
    for (;;) { }        /* not reached - exit does not come back */
}

int ModuleMain(void)
{
    int i;

    Write("hello from ring 3\n");

    Write("  thread id ");
    WriteNumber((unsigned)Syscall(SYS_GETTID, 0, 0, 0));
    Write(" at ");
    WriteNumber((unsigned)Syscall(SYS_GETMS, 0, 0, 0));
    Write(" ms\n");

    for (i = 0; i < 3; i++) {
        Syscall(SYS_SLEEP, 250, 0, 0);
        Write("  tick ");
        WriteNumber((unsigned)(i + 1));
        Write(" at ");
        WriteNumber((unsigned)Syscall(SYS_GETMS, 0, 0, 0));
        Write(" ms\n");
    }

    Write("ring 3 module done\n");
    Exit(0);
    return 0;
}
