/* serial - a 16550 UART driver in ring 3, on COM1.
 *
 * The second driver, and the point of writing it is that it proves the
 * pattern generalises: a different device, a different interrupt line,
 * and not one line of kernel code had to change to support it.
 *
 * It is also a debug channel that survives the terminal moving out of
 * the kernel later. */

#include <os/syscall.h>

#define COM1            0x3F8
#define REG_DATA        0
#define REG_IER         1
#define REG_FIFO        2
#define REG_LCR         3
#define REG_MCR         4
#define REG_LSR         5

#define LSR_DATA_READY  0x01
#define LSR_TX_EMPTY    0x20
#define COM1_IRQ        4

static void Out(unsigned reg, unsigned char value)
{
    SysOutB((unsigned short)(COM1 + reg), value);
}

static unsigned char In(unsigned reg)
{
    return SysInB((unsigned short)(COM1 + reg));
}

static void SerialPutChar(char c)
{
    /* Wait for the holding register to empty. Writing before it does
     * silently drops the previous byte. */
    while (!(In(REG_LSR) & LSR_TX_EMPTY)) { }
    Out(REG_DATA, (unsigned char)c);
}

int ModuleMain(void)
{
    int Irq, Output, Input;
    unsigned Received = 0;

    SysPrint("[serial] starting in ring 3, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    if (SysIoRequest(COM1, 8) != SYSCALL_OK) {
        SysPrintLine("[serial] denied io ports");
        SysExit(1);
    }

    /* 38400 8N1. The divisor latch is reached by setting bit 7 of the
     * line control register, which overlays the first two registers -
     * so it must be cleared again before anything else is written. */
    Out(REG_IER, 0x00);
    Out(REG_LCR, 0x80);
    Out(REG_DATA, 0x03);        /* divisor low  */
    Out(REG_IER, 0x00);         /* divisor high */
    Out(REG_LCR, 0x03);         /* 8 bits, no parity, one stop */
    Out(REG_FIFO, 0xC7);        /* enable and clear the fifos  */
    Out(REG_MCR, 0x0B);         /* DTR, RTS, OUT2              */

    /* OUT2 above is not optional: on a PC the UART's interrupt line is
     * gated through it, so without it IRQ 4 never reaches the PIC no
     * matter what the IER says. */

    Output = SysPipeCreate(256, PIPE_FLAG_NOBLOCK_WRITE);
    if (Output < 0 || SysRegisterName("serial-in", Output) != SYSCALL_OK) {
        SysPrintLine("[serial] could not publish 'serial-in'");
        SysExit(1);
    }
    Input = SysPipeCreate(256, 0);
    if (Input < 0 || SysRegisterName("serial-out", Input) != SYSCALL_OK) {
        SysPrintLine("[serial] could not publish 'serial-out'");
        SysExit(1);
    }

    while (In(REG_LSR) & LSR_DATA_READY) {
        (void)In(REG_DATA);
    }

    Irq = SysIrqRegister(COM1_IRQ);
    if (Irq < 0) {
        SysPrintLine("[serial] could not claim irq 4");
        SysExit(1);
    }
    Out(REG_IER, 0x01);         /* interrupt on received data */

    SysPrintLine("[serial] ready on COM1, irq 4");
    SerialPutChar('\r');
    SerialPutChar('\n');
    {
        const char *Banner = "TheTaaJ serial driver, ring 3\r\n";
        unsigned i;
        for (i = 0; Banner[i]; i++) {
            SerialPutChar(Banner[i]);
        }
    }

    for (;;) {
        if (SysIrqWait(Irq) < 0) {
            break;
        }

        /* Drain everything: the line is masked until the ack, so bytes
         * that arrived while this process waited to be scheduled are
         * still in the fifo. */
        while (In(REG_LSR) & LSR_DATA_READY) {
            char c = (char)In(REG_DATA);
            SysPipeWrite(Output, &c, 1);
            SerialPutChar(c);       /* echo back to the terminal */
            Received++;
        }

        SysIrqAck(Irq);
    }

    SysExit(0);
    return 0;
}
