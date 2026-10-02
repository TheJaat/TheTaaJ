/* fat32 - the file service.
 *
 * A client of the disk service and a server to everyone else. It owns
 * no hardware at all: it has no io ports, no interrupt, no device
 * memory. A bug here cannot touch the disk controller, and a bug in the
 * disk driver cannot corrupt the file service's state - they are
 * separate address spaces that exchange messages.
 *
 * That separation is the thing a monolithic kernel cannot offer, and it
 * costs one message round trip per block. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/disksrv.h>
#include <os/fssrv.h>
#include "fat32core.h"

static int DiskEndpoint = -1;
static unsigned DiskWindow = 0;     /* the disk's shared region, mapped here */
static unsigned FsWindow = 0;       /* our own, for clients */
static Fat32Volume_t Volume;

/* DiskRead
 * One synchronous call per request. The sectors land in the disk
 * service's shared window, which is mapped into this process, so the
 * data itself is never copied between the two. */
static int DiskRead(void *Context, unsigned Lba, unsigned Count, void *Buffer)
{
    DiskRequest_t Request;
    DiskResult_t Result;
    unsigned char *Out = (unsigned char*)Buffer;
    unsigned i;

    (void)Context;

    if (Count > DISK_WINDOW_SECTORS) {
        return -1;
    }
    /* A zero window means the mapping failed and was not noticed. Copying
     * from it is a null dereference at whatever offset the loop reaches,
     * which is a page fault several frames away from the real mistake. */
    if (DiskWindow == 0) {
        SysPrintLine("[fat32] the disk window is not mapped");
        return -1;
    }

    Request.Lba          = Lba;
    Request.Count        = Count;
    Request.WindowOffset = 0;

    Result.Status = -1;
    if (SysCallTimed(DiskEndpoint, DISK_OP_READ, &Request, sizeof(Request),
            &Result, sizeof(Result), 5000) < (int)sizeof(Result)) {
        return -1;
    }
    if (Result.Status != 0) {
        return -1;
    }

    /* The caller wants it in its own buffer - fat32core works on plain
     * memory and knows nothing about windows. */
    for (i = 0; i < Count * DISK_SECTOR_SIZE; i++) {
        Out[i] = ((unsigned char*)DiskWindow)[i];
    }
    return 0;
}

static void CopyName(char *Dst, const char *Src, int Max)
{
    int i;
    for (i = 0; i < Max - 1 && Src[i] != '\0'; i++) {
        Dst[i] = Src[i];
    }
    Dst[i] = '\0';
}

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];
    int Endpoint, Shm, DiskShm = -1;
    int Attempt;

    SysPrint("[fat32] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    /* Wait for the disk driver: it is a separate process and the
     * scheduler decides who runs first. */
    for (Attempt = 0; Attempt < 40; Attempt++) {
        if (SysLookup(DISKSRV_NAME, &DiskEndpoint, &DiskShm) == 0) {
            break;
        }
        SysSleep(50);
    }
    if (DiskEndpoint < 0) {
        SysPrintLine("[fat32] no disk service");
        SysExit(1);
    }

    DiskWindow = SysShmMap(DiskShm);
    if (DiskWindow == 0) {
        SysPrint("[fat32] could not map the disk window (shm handle ");
        SysPrintNumber((unsigned)DiskShm);
        SysPrintLine(")");
        SysExit(1);
    }
    SysPrint("[fat32] disk window at 0x");
    SysPrintNumber(DiskWindow);
    SysPrint("\n");

    {
        int Status = Fat32Mount(&Volume, DiskRead, 0);

        if (Status != FAT32_OK) {
            SysPrint("[fat32] cannot mount: ");
            SysPrintLine(Fat32MountError(Status));

            /* Dump the first bytes of sector 0. A disk that is being
             * read correctly but is not FAT32 looks completely
             * different from one whose driver is returning nothing, and
             * sixteen bytes tells them apart immediately. */
            {
                unsigned char Probe[512];
                int i;

                if (DiskRead(0, 0, 1, Probe) == 0) {
                    SysPrint("[fat32] sector 0 begins:");
                    for (i = 0; i < 16; i++) {
                        SysPrint(" ");
                        SysPrintNumber(Probe[i]);
                    }
                    SysPrint("\n[fat32] bytes 510,511 are ");
                    SysPrintNumber(Probe[510]);
                    SysPrint(",");
                    SysPrintNumber(Probe[511]);
                    SysPrintLine(" (expect 85,170)");
                }
                else {
                    SysPrintLine("[fat32] sector 0 could not be read at all");
                }
            }
            SysExit(1);
        }
    }

    SysPrint("[fat32] mounted: ");
    SysPrintNumber(Volume.TotalClusters);
    SysPrint(" clusters, ");
    SysPrintNumber(Volume.SectorsPerCluster);
    SysPrint(" sectors each, root at cluster ");
    SysPrintNumber(Volume.RootCluster);
    SysPrint("\n");

    Endpoint = SysEndpointCreate();
    Shm = SysShmCreate(FS_WINDOW_BYTES);
    if (Endpoint < 0 || Shm < 0) {
        SysPrintLine("[fat32] no endpoint or window");
        SysExit(1);
    }
    FsWindow = SysShmMap(Shm);

    if (SysPublish(FSSRV_NAME, Endpoint, Shm) != 0) {
        SysPrintLine("[fat32] the registry refused 'fs'");
        SysExit(1);
    }
    SysPrintLine("[fat32] published as 'fs'");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length = SysRecv(Endpoint, Message, sizeof(Message),
                             &Opcode, &Badge);

        if (Length < 0) {
            break;
        }

        switch (Opcode) {
            case FS_OP_LIST: {
                FsList_t *q = (FsList_t*)Message;
                FsListResult_t r;
                Fat32File_t Files[FS_LIST_MAX];
                int n, i;

                n = Fat32ListDirectory(&Volume,
                        (Length >= (int)sizeof(FsList_t)) ? q->Cluster : 0,
                        Files, FS_LIST_MAX);

                r.Status = (n < 0) ? -1 : 0;
                r.Count  = (n < 0) ? 0 : n;
                for (i = 0; i < r.Count; i++) {
                    CopyName(r.Entries[i].Name, Files[i].Name, FS_NAME_MAX);
                    r.Entries[i].Size        = Files[i].Size;
                    r.Entries[i].Cluster     = Files[i].FirstCluster;
                    r.Entries[i].IsDirectory = Files[i].IsDirectory;
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_STAT: {
                FsPath_t *q = (FsPath_t*)Message;
                FsStat_t r;
                Fat32File_t File;

                r.Status = -1;
                if (Length >= (int)sizeof(FsPath_t)
                    && Fat32Find(&Volume, 0, q->Name, &File)) {
                    CopyName(r.Entry.Name, File.Name, FS_NAME_MAX);
                    r.Entry.Size        = File.Size;
                    r.Entry.Cluster     = File.FirstCluster;
                    r.Entry.IsDirectory = File.IsDirectory;
                    r.Status = 0;
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_READ: {
                FsRead_t *q = (FsRead_t*)Message;
                FsReadResult_t r;
                Fat32File_t File;
                int n;

                r.Status = -1;
                r.Length = 0;

                if (Length >= (int)sizeof(FsRead_t)
                    && Fat32Find(&Volume, 0, q->Name, &File)) {
                    unsigned Want = q->Length;

                    if (Want > FS_WINDOW_BYTES) {
                        Want = FS_WINDOW_BYTES;
                    }
                    n = Fat32ReadFile(&Volume, &File, q->Offset,
                                      (void*)FsWindow, Want);
                    if (n >= 0) {
                        r.Status = 0;
                        r.Length = (unsigned)n;
                    }
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
