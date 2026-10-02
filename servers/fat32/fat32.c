/* fat32 - the file service.
 *
 * A client of the disk service and a server to everyone else. It owns no
 * hardware: no ports, no interrupt, no device memory. A bug here cannot
 * touch the disk controller, and a bug in the disk driver cannot corrupt
 * this process's state. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/disksrv.h>
#include <os/fssrv.h>
#include "fat32core.h"

#define HANDLE_MAX  8

typedef struct _OpenFile {
    Fat32File_t File;
    unsigned    Owner;          /* badge of the process that opened it */
    unsigned    Flags;
    int         Used;
} OpenFile_t;

static OpenFile_t Handles[HANDLE_MAX];
static int DiskEndpoint = -1;
static unsigned DiskWindow = 0;
static unsigned FsWindow = 0;
static Fat32Volume_t Volume;

static int DiskIo(unsigned Lba, unsigned Count, void *Buffer, int Write)
{
    DiskRequest_t Request;
    DiskResult_t Result;
    unsigned char *Data = (unsigned char*)Buffer;
    unsigned i;

    if (Count > DISK_WINDOW_SECTORS || DiskWindow == 0) {
        return -1;
    }

    if (Write) {
        for (i = 0; i < Count * DISK_SECTOR_SIZE; i++) {
            ((unsigned char*)DiskWindow)[i] = Data[i];
        }
    }

    Request.Lba = Lba; Request.Count = Count; Request.WindowOffset = 0;
    Result.Status = -1;

    if (SysCallTimed(DiskEndpoint, Write ? DISK_OP_WRITE : DISK_OP_READ,
            &Request, sizeof(Request), &Result, sizeof(Result), 5000)
        < (int)sizeof(Result)) {
        return -1;
    }
    if (Result.Status != 0) {
        return -1;
    }

    if (!Write) {
        for (i = 0; i < Count * DISK_SECTOR_SIZE; i++) {
            Data[i] = ((unsigned char*)DiskWindow)[i];
        }
    }
    return 0;
}

static int DiskReadCb(void *C, unsigned Lba, unsigned N, void *B)
{ (void)C; return DiskIo(Lba, N, B, 0); }

static int DiskWriteCb(void *C, unsigned Lba, unsigned N, const void *B)
{ (void)C; return DiskIo(Lba, N, (void*)B, 1); }

static void CopyName(char *Dst, const char *Src, int Max)
{
    int i;
    for (i = 0; i < Max - 1 && Src[i] != '\0'; i++) { Dst[i] = Src[i]; }
    Dst[i] = '\0';
}

/* HandleFor
 * Looks a handle up and checks it belongs to the caller.
 *
 * Without the badge check one process could read or - far worse - write
 * through another's handle just by guessing a small integer. The badge
 * is the kernel's word for who is calling, so this is a real check
 * rather than a convention. */
static OpenFile_t *HandleFor(int Index, unsigned Badge)
{
    if (Index < 0 || Index >= HANDLE_MAX) { return 0; }
    if (!Handles[Index].Used) { return 0; }
    if (Handles[Index].Owner != Badge) { return 0; }
    return &Handles[Index];
}

static void CloseOwnedBy(unsigned Badge)
{
    int i;
    for (i = 0; i < HANDLE_MAX; i++) {
        if (Handles[i].Used && Handles[i].Owner == Badge) {
            Handles[i].Used = 0;
        }
    }
}

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];
    int Endpoint, Shm, DiskShm = -1;
    int Attempt, Status;

    SysPrint("[fat32] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    for (Attempt = 0; Attempt < 40; Attempt++) {
        if (SysLookup(DISKSRV_NAME, &DiskEndpoint, &DiskShm) == 0) { break; }
        SysSleep(50);
    }
    if (DiskEndpoint < 0) {
        SysPrintLine("[fat32] no disk service");
        SysExit(1);
    }

    DiskWindow = SysShmMap(DiskShm);
    if (DiskWindow == 0) {
        SysPrintLine("[fat32] could not map the disk window");
        SysExit(1);
    }

    Status = Fat32Mount(&Volume, DiskReadCb, DiskWriteCb, 0);
    if (Status != FAT32_OK) {
        SysPrint("[fat32] cannot mount: ");
        SysPrintLine(Fat32Error(Status));
        SysExit(1);
    }

    SysPrint("[fat32] mounted read-write: ");
    SysPrintNumber(Volume.TotalClusters);
    SysPrint(" clusters, ");
    SysPrintNumber(Volume.SectorsPerCluster);
    SysPrintLine(" sectors each");

    Endpoint = SysEndpointCreate();
    Shm = SysShmCreate(FS_WINDOW_BYTES);
    if (Endpoint < 0 || Shm < 0) {
        SysPrintLine("[fat32] no endpoint or window");
        SysExit(1);
    }
    FsWindow = SysShmMap(Shm);
    if (FsWindow == 0) {
        SysPrintLine("[fat32] could not map my own window");
        SysExit(1);
    }

    if (SysPublish(FSSRV_NAME, Endpoint, Shm) != 0) {
        SysPrintLine("[fat32] the registry refused 'fs'");
        SysExit(1);
    }
    SysPrintLine("[fat32] published as 'fs'");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length = SysRecv(Endpoint, Message, sizeof(Message),
                             &Opcode, &Badge);

        if (Length < 0) { break; }

        switch (Opcode) {
            case FS_OP_LIST: {
                FsList_t *q = (FsList_t*)Message;
                FsListResult_t r;
                Fat32File_t Files[FS_LIST_MAX];
                Fat32File_t Dir;
                unsigned Cluster = 0;
                int n, i;

                r.Status = -1; r.Count = 0;

                if (Length >= (int)sizeof(FsList_t)) {
                    q->Path[FS_PATH_MAX - 1] = '\0';
                    if (q->Path[0] == '\0'
                        || Fat32Resolve(&Volume, q->Path, &Dir) == FAT32_OK) {
                        if (q->Path[0] != '\0') {
                            Cluster = Dir.IsDirectory ? Dir.FirstCluster : 0;
                        }
                        n = Fat32ListDirectory(&Volume, Cluster,
                                               Files, FS_LIST_MAX);
                        if (n >= 0) {
                            r.Status = 0;
                            r.Count = n;
                            for (i = 0; i < n; i++) {
                                CopyName(r.Entries[i].Name, Files[i].Name,
                                         FS_NAME_MAX);
                                r.Entries[i].Size = Files[i].Size;
                                r.Entries[i].IsDirectory = Files[i].IsDirectory;
                            }
                        }
                    }
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_OPEN: {
                FsOpen_t *q = (FsOpen_t*)Message;
                FsHandleResult_t r;
                Fat32File_t File;
                int i, Slot = -1;

                r.Status = -1; r.Handle = -1; r.Size = 0;

                if (Length < (int)sizeof(FsOpen_t)) {
                    SysReply(&r, sizeof(r));
                    break;
                }
                q->Path[FS_PATH_MAX - 1] = '\0';

                for (i = 0; i < HANDLE_MAX; i++) {
                    if (!Handles[i].Used) { Slot = i; break; }
                }
                if (Slot < 0) {
                    SysReply(&r, sizeof(r));
                    break;
                }

                Status = Fat32Resolve(&Volume, q->Path, &File);
                if (Status == FAT32_ERR_NOTFOUND
                    && (q->Flags & FS_OPEN_CREATE)) {
                    Status = Fat32Create(&Volume, q->Path, &File);
                }
                if (Status != FAT32_OK) {
                    SysReply(&r, sizeof(r));
                    break;
                }

                Handles[Slot].File  = File;
                Handles[Slot].Owner = Badge;
                Handles[Slot].Flags = q->Flags;
                Handles[Slot].Used  = 1;

                r.Status = 0;
                r.Handle = Slot;
                r.Size   = File.Size;
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_CLOSE: {
                FsHandle_t *q = (FsHandle_t*)Message;
                FsStatus_t r;
                OpenFile_t *H;

                r.Status = -1;
                if (Length >= (int)sizeof(FsHandle_t)) {
                    H = HandleFor(q->Handle, Badge);
                    if (H) { H->Used = 0; r.Status = 0; }
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_READ:
            case FS_OP_WRITE: {
                FsIo_t *q = (FsIo_t*)Message;
                FsIoResult_t r;
                OpenFile_t *H;
                int n;

                r.Status = -1; r.Length = 0;

                if (Length < (int)sizeof(FsIo_t)) {
                    SysReply(&r, sizeof(r));
                    break;
                }
                H = HandleFor(q->Handle, Badge);
                if (!H) {
                    SysReply(&r, sizeof(r));
                    break;
                }
                /* Clamp into the window before touching it: a length the
                 * caller chose must never decide how far we write into
                 * memory shared with other clients. */
                if (q->WindowOffset >= FS_WINDOW_BYTES
                    || q->Length > (FS_WINDOW_BYTES - q->WindowOffset)) {
                    SysReply(&r, sizeof(r));
                    break;
                }

                if (Opcode == FS_OP_READ) {
                    if (!(H->Flags & FS_OPEN_READ)) {
                        SysReply(&r, sizeof(r));
                        break;
                    }
                    n = Fat32ReadFile(&Volume, &H->File, q->Offset,
                            (void*)(FsWindow + q->WindowOffset), q->Length);
                }
                else {
                    if (!(H->Flags & FS_OPEN_WRITE)) {
                        SysReply(&r, sizeof(r));
                        break;
                    }
                    n = Fat32WriteFile(&Volume, &H->File, q->Offset,
                            (void*)(FsWindow + q->WindowOffset), q->Length);
                }

                if (n >= 0) {
                    r.Status = 0;
                    r.Length = (unsigned)n;
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_STAT: {
                FsPath_t *q = (FsPath_t*)Message;
                FsStatResult_t r;
                Fat32File_t File;

                r.Status = -1;
                if (Length >= (int)sizeof(FsPath_t)) {
                    q->Path[FS_PATH_MAX - 1] = '\0';
                    if (Fat32Resolve(&Volume, q->Path, &File) == FAT32_OK) {
                        CopyName(r.Entry.Name, File.Name, FS_NAME_MAX);
                        r.Entry.Size = File.Size;
                        r.Entry.IsDirectory = File.IsDirectory;
                        r.Status = 0;
                    }
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_DELETE: {
                FsPath_t *q = (FsPath_t*)Message;
                FsStatus_t r;

                r.Status = -1;
                if (Length >= (int)sizeof(FsPath_t)) {
                    q->Path[FS_PATH_MAX - 1] = '\0';
                    r.Status = (Fat32Delete(&Volume, q->Path) == FAT32_OK)
                             ? 0 : -1;
                    if (r.Status == 0) {
                        /* Any handle still open on that path now refers
                         * to a file that no longer exists. Closing them
                         * is cruder than a real reference count, but it
                         * is far better than leaving a handle that
                         * writes into freed clusters. */
                        int h;
                        for (h = 0; h < HANDLE_MAX; h++) {
                            if (Handles[h].Used
                                && Handles[h].File.EntrySector != 0) {
                                Fat32File_t Check;
                                if (Fat32Resolve(&Volume, q->Path, &Check)
                                    != FAT32_OK) {
                                    Handles[h].Used = 0;
                                }
                            }
                        }
                    }
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_TRUNCATE: {
                FsTruncate_t *q = (FsTruncate_t*)Message;
                FsStatus_t r;
                Fat32File_t File;

                r.Status = -1;
                if (Length >= (int)sizeof(FsTruncate_t)) {
                    q->Path[FS_PATH_MAX - 1] = '\0';
                    if (Fat32Resolve(&Volume, q->Path, &File) == FAT32_OK) {
                        r.Status = (Fat32Truncate(&Volume, &File, q->Size)
                                    == FAT32_OK) ? 0 : -1;
                    }
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_SYNC: {
                FsStatus_t r;
                r.Status = (Fat32Flush(&Volume) == FAT32_OK) ? 0 : -1;
                SysReply(&r, sizeof(r));
                break;
            }

            default:
                SysReply(0, 0);
                break;
        }

        /* A client that exits leaves its handles behind. Nothing tells
         * us it has gone, so check on each request - it is a table of
         * eight. */
        if (!SysProcessAlive((int)Badge)) {
            CloseOwnedBy(Badge);
        }
    }

    Fat32Flush(&Volume);
    SysExit(0);
    return 0;
}
