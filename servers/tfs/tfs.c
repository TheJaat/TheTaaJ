/* tfs - the TaajFS file service.
 *
 * It speaks exactly the protocol fat32.mod speaks, on a second drive.
 * Nothing above it knows which filesystem it is talking to, which is
 * the point of having built the interface against FAT first: a second
 * implementation is the only real test of whether an interface is an
 * interface or just one program's habits. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/disksrv.h>
#include <os/fssrv.h>
#include "tfscore.h"

#define TFSSRV_NAME     "tfs"
#define HANDLE_MAX      8
#define TFS_DRIVE       1       /* the slave on the primary channel */

typedef struct _OpenFile {
    TfsFile_t    File;
    char         Name[TFS_NAME_MAX];
    unsigned     Owner;
    unsigned     Flags;
    int          Used;
} OpenFile_t;

static OpenFile_t Handles[HANDLE_MAX];
static int DiskEndpoint = -1;
static unsigned DiskWindow = 0;
static unsigned FsWindow = 0;
/* The volume carries the scratch buffers, so it is large - about 20 KB.
 * A global, not a local: on the stack it would be most of a ring-3
 * stack by itself. */
static TfsVolume_t Volume;

/* A TaajFS block is eight sectors, and the disk window holds sixteen,
 * so one block is one request. */
static int DiskIo(unsigned Block, void *Buffer, int Write)
{
    DiskRequest_t Request;
    DiskResult_t Result;
    unsigned char *Data = (unsigned char*)Buffer;
    unsigned i;

    if (DiskWindow == 0) { return -1; }

    if (Write) {
        for (i = 0; i < TFS_BLOCK_SIZE; i++) {
            ((unsigned char*)DiskWindow)[i] = Data[i];
        }
    }

    Request.Drive        = TFS_DRIVE;
    Request.Lba          = Block * TFS_SECTORS_PER_BLOCK;
    Request.Count        = TFS_SECTORS_PER_BLOCK;
    Request.WindowOffset = 0;
    Result.Status = -1;

    if (SysCallTimed(DiskEndpoint, Write ? DISK_OP_WRITE : DISK_OP_READ,
            &Request, sizeof(Request), &Result, sizeof(Result), 5000)
        < (int)sizeof(Result) || Result.Status != 0) {
        return -1;
    }

    if (!Write) {
        for (i = 0; i < TFS_BLOCK_SIZE; i++) {
            Data[i] = ((unsigned char*)DiskWindow)[i];
        }
    }
    return 0;
}

static int ReadCb(void *C, unsigned B, void *Buf)
{ (void)C; return DiskIo(B, Buf, 0); }

static int WriteCb(void *C, unsigned B, const void *Buf)
{ (void)C; return DiskIo(B, (void*)Buf, 1); }

static void CopyName(char *Dst, const char *Src, int Max)
{
    int i;
    for (i = 0; i < Max - 1 && Src[i] != '\0'; i++) { Dst[i] = Src[i]; }
    Dst[i] = '\0';
}

static OpenFile_t *HandleFor(int Index, unsigned Badge)
{
    if (Index < 0 || Index >= HANDLE_MAX) { return 0; }
    if (!Handles[Index].Used) { return 0; }
    if (Handles[Index].Owner != Badge) { return 0; }
    return &Handles[Index];
}

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];
    int Endpoint, Shm, DiskShm = -1;
    int Attempt, Status;

    SysPrint("[tfs] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    for (Attempt = 0; Attempt < 40; Attempt++) {
        if (SysLookup(DISKSRV_NAME, &DiskEndpoint, &DiskShm) == 0) { break; }
        SysSleep(50);
    }
    if (DiskEndpoint < 0) {
        SysPrintLine("[tfs] no disk service");
        SysExit(1);
    }

    DiskWindow = SysShmMap(DiskShm);
    if (DiskWindow == 0) {
        SysPrintLine("[tfs] could not map the disk window");
        SysExit(1);
    }

    Status = TfsMount(&Volume, ReadCb, WriteCb, 0);
    if (Status != TFS_OK) {
        SysPrint("[tfs] cannot mount drive 1: ");
        SysPrintLine(TfsError(Status));
        SysExit(1);
    }

    SysPrint("[tfs] mounted: generation ");
    SysPrintNumber(Volume.Super.Generation);
    SysPrint(", slot ");
    SysPrint(Volume.SuperSlot == 0 ? "A" : "B");
    SysPrint(", ");
    SysPrintNumber(TfsFreeBlocks(&Volume));
    SysPrint(" of ");
    SysPrintNumber(Volume.Super.TotalBlocks);
    SysPrintLine(" blocks free");

    Endpoint = SysEndpointCreate();
    Shm = SysShmCreate(FS_WINDOW_BYTES);
    if (Endpoint < 0 || Shm < 0) {
        SysPrintLine("[tfs] no endpoint or window");
        SysExit(1);
    }
    FsWindow = SysShmMap(Shm);
    if (FsWindow == 0) {
        SysPrintLine("[tfs] could not map my own window");
        SysExit(1);
    }

    if (SysPublish(TFSSRV_NAME, Endpoint, Shm) != 0) {
        SysPrintLine("[tfs] the registry refused 'tfs'");
        SysExit(1);
    }
    SysPrintLine("[tfs] published as 'tfs'");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length = SysRecv(Endpoint, Message, sizeof(Message),
                             &Opcode, &Badge);

        if (Length < 0) { break; }

        switch (Opcode) {
            case FS_OP_LIST: {
                FsListResult_t r;
                FsEntry_t *Out = (FsEntry_t*)FsWindow;
                TfsFile_t Files[FS_LIST_MAX];
                int n, i;

                r.Status = -1; r.Count = 0; r.Truncated = 0;

                n = TfsList(&Volume, Files, FS_LIST_MAX);
                if (n >= 0) {
                    for (i = 0; i < n; i++) {
                        CopyName(Out[i].Name, Files[i].Name, FS_NAME_MAX);
                        Out[i].Size = Files[i].Size;
                        Out[i].IsDirectory =
                            (Files[i].Type == TFS_TYPE_DIR) ? 1 : 0;
                    }
                    r.Status = 0;
                    r.Count = n;
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_OPEN: {
                FsOpen_t *q = (FsOpen_t*)Message;
                FsHandleResult_t r;
                TfsFile_t File;
                int i, Slot = -1;

                r.Status = -1; r.Handle = -1; r.Size = 0;

                if (Length < (int)sizeof(FsOpen_t)) {
                    SysReply(&r, sizeof(r)); break;
                }
                q->Path[FS_PATH_MAX - 1] = '\0';

                for (i = 0; i < HANDLE_MAX; i++) {
                    if (!Handles[i].Used) { Slot = i; break; }
                }
                if (Slot < 0) { SysReply(&r, sizeof(r)); break; }

                Status = TfsLookup(&Volume, q->Path, &File);
                if (Status == TFS_ERR_NOTFOUND && (q->Flags & FS_OPEN_CREATE)) {
                    Status = TfsCreate(&Volume, q->Path);
                    if (Status == TFS_OK) {
                        Status = TfsLookup(&Volume, q->Path, &File);
                    }
                }
                if (Status != TFS_OK) { SysReply(&r, sizeof(r)); break; }

                Handles[Slot].File  = File;
                CopyName(Handles[Slot].Name, File.Name, TFS_NAME_MAX);
                Handles[Slot].Owner = Badge;
                Handles[Slot].Flags = q->Flags;
                Handles[Slot].Used  = 1;

                r.Status = 0; r.Handle = Slot; r.Size = File.Size;
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
                    SysReply(&r, sizeof(r)); break;
                }
                H = HandleFor(q->Handle, Badge);
                if (!H) { SysReply(&r, sizeof(r)); break; }

                if (q->WindowOffset >= FS_WINDOW_BYTES
                    || q->Length > (FS_WINDOW_BYTES - q->WindowOffset)) {
                    SysReply(&r, sizeof(r)); break;
                }

                if (Opcode == FS_OP_READ) {
                    if (!(H->Flags & FS_OPEN_READ)) {
                        SysReply(&r, sizeof(r)); break;
                    }
                    n = TfsRead(&Volume, &H->File, q->Offset,
                                (void*)(FsWindow + q->WindowOffset), q->Length);
                }
                else {
                    if (!(H->Flags & FS_OPEN_WRITE)) {
                        SysReply(&r, sizeof(r)); break;
                    }
                    n = TfsWriteFile(&Volume, H->Name, q->Offset,
                                (void*)(FsWindow + q->WindowOffset), q->Length);
                    /* Every write commits, so the handle's idea of the
                     * file is stale the moment it returns. Refresh it,
                     * or an append would keep writing at the old size. */
                    if (n >= 0) {
                        TfsLookup(&Volume, H->Name, &H->File);
                    }
                }

                if (n >= 0) { r.Status = 0; r.Length = (unsigned)n; }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_STAT: {
                FsPath_t *q = (FsPath_t*)Message;
                FsStatResult_t r;
                TfsFile_t File;

                r.Status = -1;
                if (Length >= (int)sizeof(FsPath_t)) {
                    q->Path[FS_PATH_MAX - 1] = '\0';
                    if (TfsLookup(&Volume, q->Path, &File) == TFS_OK) {
                        CopyName(r.Entry.Name, File.Name, FS_NAME_MAX);
                        r.Entry.Size = File.Size;
                        r.Entry.IsDirectory =
                            (File.Type == TFS_TYPE_DIR) ? 1 : 0;
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
                    r.Status = (TfsDelete(&Volume, q->Path) == TFS_OK) ? 0 : -1;
                }
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_SYNC: {
                FsStatus_t r;
                /* Nothing to flush: every operation already committed.
                 * That is not a shortcut, it is the design - there is no
                 * window in which the disk disagrees with what a caller
                 * has been told succeeded. */
                r.Status = 0;
                SysReply(&r, sizeof(r));
                break;
            }

            case FS_OP_MKDIR:
            case FS_OP_TRUNCATE: {
                FsStatus_t r;
                r.Status = -1;      /* not implemented for TaajFS yet */
                SysReply(&r, sizeof(r));
                break;
            }

            default:
                SysReply(0, 0);
                break;
        }

        if (!SysProcessAlive((int)Badge)) {
            int h;
            for (h = 0; h < HANDLE_MAX; h++) {
                if (Handles[h].Used && Handles[h].Owner == Badge) {
                    Handles[h].Used = 0;
                }
            }
        }
    }

    SysExit(0);
    return 0;
}
