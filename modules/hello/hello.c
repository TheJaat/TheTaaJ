/* hello - a loadable module for TheTaaJ.
 *
 * Built as a relocatable object (.o) and packed into the ramdisk. It is
 * NOT linked against the kernel: every function it calls below is an
 * undefined symbol, resolved at load time against the kernel's export
 * table. Ask for something that is not exported and the load is refused
 * rather than the call landing at address 0.
 *
 * ModuleMain is the entry point the loader looks for by name. */

/* Declared, never defined here - these are the undefined references the
 * loader resolves. */
extern int    printf(const char *format, ...);
extern void   LogInformation(const char *system, const char *message, ...);
extern void  *kmalloc(unsigned int size);
extern void   kfree(void *p);
extern void   SleepMs(unsigned int ms);
extern unsigned int TimersGetSystemMs(void);
extern unsigned int ThreadingGetCurrentThreadId(void);
extern void  *memset(void *dest, int c, unsigned int count);

/* Static data, to prove .data and .rodata relocate correctly. A string
 * literal referenced from code needs an R_386_32 against .rodata; get
 * that wrong and this prints garbage. */
static const char *Greeting = "hello from a loadable module";
static int CallCount = 0;

/* .bss, to prove NOBITS sections are allocated and zeroed. */
static int Zeroes[16];

static int SumZeroes(void)
{
    int i, sum = 0;
    for (i = 0; i < 16; i++) {
        sum += Zeroes[i];
    }
    return sum;
}

int ModuleMain(void)
{
    int i;
    char *Scratch;

    CallCount++;

    printf("%s\n", Greeting);
    printf("  running as thread %u at %u ms\n",
        ThreadingGetCurrentThreadId(), TimersGetSystemMs());
    printf("  .data works: CallCount = %d\n", CallCount);
    printf("  .bss zeroed: sum = %d (expect 0)\n", SumZeroes());

    /* Calling back into the kernel heap from module code. */
    Scratch = (char*)kmalloc(64);
    if (Scratch == 0) {
        printf("  kmalloc failed\n");
        return -1;
    }
    memset(Scratch, 'x', 63);
    Scratch[63] = 0;
    printf("  kmalloc gave 0x%x, first bytes: %c%c%c\n",
        (unsigned int)Scratch, Scratch[0], Scratch[1], Scratch[2]);
    kfree(Scratch);

    for (i = 0; i < 3; i++) {
        SleepMs(300);
        LogInformation("hello", "tick %d at %u ms", i + 1, TimersGetSystemMs());
    }

    printf("hello module done\n");
    return 0;
}
