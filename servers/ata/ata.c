/* ata - an ATA PIO disk driver in ring 3.
 *
 * Nothing new was needed in the kernel for this. Ports 0x1F0-0x1F7 and
 * 0x3F6 plus IRQ 14 are all grantable already, which is the point: a
 * whole class of device added entirely in user space.
 *
 * PIO rather than DMA deliberately. DMA needs physically contiguous
 * buffers and a way to tell the device where they are, neither of which
 * exists yet. PIO is slower and entirely sufficient to get a filesystem
 * working. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/disksrv.h>

#define ATA_DATA            0x1F0
#define ATA_SECTOR_COUNT    0x1F2
#define ATA_LBA_LOW         0x1F3
#define ATA_LBA_MID         0x1F4
#define ATA_LBA_HIGH        0x1F5
#define ATA_DRIVE           0x1F6
#define ATA_STATUS          0x1F7
#define ATA_COMMAND         0x1F7
#define ATA_CONTROL         0x3F6

#define STATUS_ERR          0x01
#define STATUS_DRQ          0x08
#define STATUS_DF           0x20
#define STATUS_BUSY         0x80

#define CMD_READ_SECTORS    0x20
#define CMD_WRITE_SECTORS   0x30
#define CMD_FLUSH_CACHE     0xE7
#define CMD_IDENTIFY        0xEC

static DiskInfo_t GlbInfo;
static unsigned GlbWindow = 0;

/* AtaDelay
 * Reading the alternate status register takes about 100ns and has no
 * side effects. Four of them is the documented way to let the drive put
 * a valid status on the bus; reading the real status register instead
 * would acknowledge a pending interrupt. */
static void AtaDelay(void)
{
    int i;
    for (i = 0; i < 4; i++) {
        (void)SysInB(ATA_CONTROL);
    }
}

static int AtaWaitNotBusy(void)
{
    unsigned Spins = 0;

    while (SysInB(ATA_STATUS) & STATUS_BUSY) {
        if (++Spins > 10000000u) {
            return -1;
        }
    }
    return 0;
}

/* AtaWaitDataRequest
 * Waits for DRQ, failing on ERR or DF rather than spinning forever. A
 * drive that has faulted never sets DRQ, so a loop testing only DRQ
 * hangs the driver on the first bad sector. */
static int AtaWaitDataRequest(void)
{
    unsigned Spins = 0;

    for (;;) {
        unsigned char Status = SysInB(ATA_STATUS);

        if (Status & (STATUS_ERR | STATUS_DF)) {
            return -1;
        }
        if (!(Status & STATUS_BUSY) && (Status & STATUS_DRQ)) {
            return 0;
        }
        if (++Spins > 10000000u) {
            return -1;
        }
    }
}

/* AtaSelect
 * Drive 0, LBA mode, top four address bits in the low nibble. The 0xE0
 * is not decoration: bit 6 selects LBA over CHS, and bits 7 and 5 are
 * required to be set. */
static void AtaSelect(unsigned Lba)
{
    SysOutB(ATA_DRIVE, (unsigned char)(0xE0 | ((Lba >> 24) & 0x0F)));
    AtaDelay();
}

static int AtaIdentify(void)
{
    unsigned short Data[256];
    int i;

    SysOutB(ATA_DRIVE, 0xA0);
    AtaDelay();
    SysOutB(ATA_SECTOR_COUNT, 0);
    SysOutB(ATA_LBA_LOW, 0);
    SysOutB(ATA_LBA_MID, 0);
    SysOutB(ATA_LBA_HIGH, 0);
    SysOutB(ATA_COMMAND, CMD_IDENTIFY);
    AtaDelay();

    if (SysInB(ATA_STATUS) == 0) {
        return -1;                  /* nothing on this channel */
    }
    if (AtaWaitDataRequest() != 0) {
        return -1;
    }

    /* One 16-bit read per word. The data register is 16 bits wide, and
     * two byte reads of it are a different operation - not a slower
     * equivalent. This is the same mistake as writing the PCI config
     * address register a byte at a time: access width is part of a
     * device's contract. */
    for (i = 0; i < 256; i++) {
        Data[i] = SysInW(ATA_DATA);
    }


    /* Words 60-61: the 28-bit LBA sector count. */
    GlbInfo.Sectors    = (unsigned)Data[60] | ((unsigned)Data[61] << 16);
    GlbInfo.SectorSize = DISK_SECTOR_SIZE;
    GlbInfo.Present    = 1;

    /* Words 27-46: the model, byte-swapped within each word. */
    for (i = 0; i < 20; i++) {
        GlbInfo.Model[i * 2]     = (char)(Data[27 + i] >> 8);
        GlbInfo.Model[i * 2 + 1] = (char)(Data[27 + i] & 0xFF);
    }
    GlbInfo.Model[40] = '\0';

    return 0;
}

static int AtaTransfer(unsigned Lba, unsigned Count, unsigned Offset,
                       int Write)
{
    unsigned char *Buffer = (unsigned char*)(GlbWindow + Offset);
    unsigned s, i;

    if (Count == 0 || Count > DISK_WINDOW_SECTORS) {
        return -1;
    }
    if ((Offset + (Count * DISK_SECTOR_SIZE)) > DISK_WINDOW_BYTES) {
        return -1;                  /* would run past the shared window */
    }
    if (AtaWaitNotBusy() != 0) {
        return -1;
    }

    AtaSelect(Lba);
    SysOutB(ATA_SECTOR_COUNT, (unsigned char)Count);
    SysOutB(ATA_LBA_LOW,  (unsigned char)(Lba & 0xFF));
    SysOutB(ATA_LBA_MID,  (unsigned char)((Lba >> 8) & 0xFF));
    SysOutB(ATA_LBA_HIGH, (unsigned char)((Lba >> 16) & 0xFF));
    SysOutB(ATA_COMMAND, Write ? CMD_WRITE_SECTORS : CMD_READ_SECTORS);

    for (s = 0; s < Count; s++) {
        if (AtaWaitDataRequest() != 0) {
            return -1;
        }

        /* One sector, one 16-bit access per word. The buffer is bytes,
         * so assemble and split explicitly rather than casting it to a
         * short pointer - the window offset is not guaranteed to be
         * even, and an unaligned 16-bit access is a different problem
         * again. */
        for (i = 0; i < DISK_SECTOR_SIZE / 2; i++) {
            unsigned Index = (s * DISK_SECTOR_SIZE) + (i * 2);

            if (Write) {
                unsigned short Word = (unsigned short)(Buffer[Index]
                    | ((unsigned short)Buffer[Index + 1] << 8));
                SysOutW(ATA_DATA, Word);
            }
            else {
                unsigned short Word = SysInW(ATA_DATA);
                Buffer[Index]     = (unsigned char)(Word & 0xFF);
                Buffer[Index + 1] = (unsigned char)(Word >> 8);
            }
        }
    }

    if (Write) {
        /* Without the flush the drive may still be holding the data in
         * its own cache, where a reset or power loss drops it. */
        SysOutB(ATA_COMMAND, CMD_FLUSH_CACHE);
        AtaWaitNotBusy();
    }

    return 0;
}

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];
    int Endpoint, Shm;

    SysPrint("[ata] starting in ring 3, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    if (SysIoRequest(ATA_DATA, 8) != SYSCALL_OK
        || SysIoRequest(ATA_CONTROL, 1) != SYSCALL_OK) {
        SysPrintLine("[ata] denied io ports");
        SysExit(1);
    }

    /* Disable the drive's interrupt: this driver polls. Polling is the
     * wrong answer for a busy disk and the right one for a first
     * version - it removes an entire class of bug while the rest of the
     * stack is being proven. */
    SysOutB(ATA_CONTROL, 0x02);

    if (AtaIdentify() != 0) {
        SysPrintLine("[ata] no drive on the primary channel");
        SysExit(1);
    }

    if (GlbInfo.Sectors == 0) {
        /* IDENTIFY answered but reported nothing. Publishing anyway
         * would hand the filesystem a device that cannot work, and the
         * failure would surface three processes away. */
        SysPrintLine("[ata] drive reports zero sectors - refusing to publish");
        SysExit(1);
    }

    SysPrint("[ata] ");
    SysPrint(GlbInfo.Model);
    SysPrint(", ");
    SysPrintNumber(GlbInfo.Sectors);
    SysPrint(" sectors (");
    SysPrintNumber(GlbInfo.Sectors / 2048);
    SysPrintLine(" MB)");

    Endpoint = SysEndpointCreate();
    Shm = SysShmCreate(DISK_WINDOW_BYTES);
    if (Endpoint < 0 || Shm < 0) {
        SysPrintLine("[ata] no endpoint or window");
        SysExit(1);
    }
    GlbWindow = SysShmMap(Shm);
    if (GlbWindow == 0) {
        SysPrintLine("[ata] could not map the window");
        SysExit(1);
    }

    if (SysPublish(DISKSRV_NAME, Endpoint, Shm) != 0) {
        SysPrintLine("[ata] the registry refused 'disk'");
        SysExit(1);
    }
    SysPrintLine("[ata] published as 'disk'");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length = SysRecv(Endpoint, Message, sizeof(Message),
                             &Opcode, &Badge);

        if (Length < 0) {
            break;
        }

        switch (Opcode) {
            case DISK_OP_INFO:
                SysReply(&GlbInfo, sizeof(GlbInfo));
                break;

            case DISK_OP_READ:
            case DISK_OP_WRITE: {
                DiskRequest_t *q = (DiskRequest_t*)Message;
                DiskResult_t r;

                if (Length < (int)sizeof(DiskRequest_t)) {
                    r.Status = -1;
                    r.Count = 0;
                }
                else {
                    r.Status = AtaTransfer(q->Lba, q->Count, q->WindowOffset,
                                           Opcode == DISK_OP_WRITE);
                    r.Count = (r.Status == 0) ? q->Count : 0;
                }
                SysReply(&r, sizeof(r));
                break;
            }

            default:
                SysReply(0, 0);
                break;
        }
    }

    SysExit(0);
    return 0;
}
