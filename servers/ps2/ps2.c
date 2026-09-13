/* ps2 - the PS/2 keyboard driver, in ring 3.
 *
 * Everything this needs from the kernel is delegated, not called:
 *
 *   SysIoRequest    opens ports 0x60-0x64 in this process's I/O bitmap,
 *                   after which in/out are ordinary instructions
 *   SysIrqRegister  asks the kernel to forward IRQ 1 here
 *   SysIrqWait      blocks until it fires
 *   SysIrqAck       re-enables the line
 *
 * The kernel does not know this is a keyboard. It masks a line, wakes a
 * process, and checks a bitmap. Everything about scancodes lives here. */

#include <os/syscall.h>

#define PS2_DATA            0x60
#define PS2_STATUS          0x64
#define PS2_OUTPUT_FULL     0x01
#define PS2_IRQ             1

static const char Map[128] = {
      0,  27, '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '=','\b',
    '\t','q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']','\n',   0,
     'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', ';','\'', '`',   0,'\\',
     'z', 'x', 'c', 'v', 'b', 'n', 'm', ',', '.', '/',   0, '*',   0, ' ',
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
     '7', '8', '9', '-', '4', '5', '6', '+', '1', '2', '3', '0', '.',   0,
};

static const char MapShift[128] = {
      0,  27, '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+','\b',
    '\t','Q', 'W', 'E', 'R', 'T', 'Y', 'U', 'I', 'O', 'P', '{', '}','\n',   0,
     'A', 'S', 'D', 'F', 'G', 'H', 'J', 'K', 'L', ':', '"', '~',   0, '|',
     'Z', 'X', 'C', 'V', 'B', 'N', 'M', '<', '>', '?',   0, '*',   0, ' ',
       0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
     '7', '8', '9', '-', '4', '5', '6', '+', '1', '2', '3', '0', '.',   0,
};

static int ShiftHeld = 0;
static int ExtendedPending = 0;

int ModuleMain(void)
{
    int Irq, Output;
    unsigned Keys = 0;

    SysPrint("[ps2] starting in ring 3, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    /* 0x60 through 0x64 - five ports. The status and data registers are
     * not adjacent but the range is small enough to grant whole. */
    if (SysIoRequest(PS2_DATA, 5) != SYSCALL_OK) {
        SysPrintLine("[ps2] denied io ports - is this running as a server?");
        SysExit(1);
    }

    Output = SysPipeCreate(256, PIPE_FLAG_NOBLOCK_WRITE);
    if (Output < 0 || SysRegisterName("keyboard", Output) != SYSCALL_OK) {
        SysPrintLine("[ps2] could not publish 'keyboard'");
        SysExit(1);
    }

    /* Drain anything left in the controller before claiming the line.
     * The 8042 will not raise IRQ 1 again while its output buffer is
     * full, so a stale byte here means the first keypress is lost and
     * the driver looks dead. */
    while (SysInB(PS2_STATUS) & PS2_OUTPUT_FULL) {
        (void)SysInB(PS2_DATA);
    }

    Irq = SysIrqRegister(PS2_IRQ);
    if (Irq < 0) {
        SysPrintLine("[ps2] could not claim irq 1");
        SysExit(1);
    }

    SysPrintLine("[ps2] ready - irq 1 forwarded, ports granted");

    for (;;) {
        if (SysIrqWait(Irq) < 0) {
            break;
        }

        /* The line is masked until the ack below, so everything that
         * arrived while this process waited to be scheduled is sitting
         * in the controller. Drain it all, or those keystrokes are
         * lost and the buffer stays full. */
        while (SysInB(PS2_STATUS) & PS2_OUTPUT_FULL) {
            unsigned char Scancode = SysInB(PS2_DATA);
            char Character;

            if (Scancode == 0xE0) {
                ExtendedPending = 1;
                continue;
            }
            if (ExtendedPending) {
                ExtendedPending = 0;
                continue;
            }
            if (Scancode & 0x80) {
                unsigned char Make = Scancode & 0x7F;
                if (Make == 0x2A || Make == 0x36) {
                    ShiftHeld = 0;
                }
                continue;
            }
            if (Scancode == 0x2A || Scancode == 0x36) {
                ShiftHeld = 1;
                continue;
            }

            Character = ShiftHeld ? MapShift[Scancode] : Map[Scancode];
            if (Character != 0) {
                SysPipeWrite(Output, &Character, 1);
                Keys++;
            }
        }

        /* Only now is the device quiet and the buffer empty. */
        SysIrqAck(Irq);
    }

    SysPrint("[ps2] stopping after ");
    SysPrintNumber(Keys);
    SysPrintLine(" keys");
    SysExit(0);
    return 0;
}
