/* fsls - exercises the file service: list, read, write, read back.
 *
 *   fsls -> fat32 -> ata -> hardware
 *
 * Four processes, four address spaces, and the only things crossing
 * between them are messages and a shared page. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/fssrv.h>

static int Fs = -1;
static unsigned Window = 0;

static void SetPath(char *Dst, const char *Src)
{
    unsigned i;
    for (i = 0; i < FS_PATH_MAX - 1 && Src[i] != '\0'; i++) { Dst[i] = Src[i]; }
    for (; i < FS_PATH_MAX; i++) { Dst[i] = '\0'; }
}

static int Open(const char *Path, unsigned Flags, unsigned *Size)
{
    FsOpen_t q;
    FsHandleResult_t r;

    SetPath(q.Path, Path);
    q.Flags = Flags;
    r.Status = -1; r.Handle = -1; r.Size = 0;

    if (SysCallTimed(Fs, FS_OP_OPEN, &q, sizeof(q), &r, sizeof(r), 5000)
        < (int)sizeof(r) || r.Status != 0) {
        return -1;
    }
    if (Size) { *Size = r.Size; }
    return r.Handle;
}

static int Io(int Handle, unsigned Offset, unsigned Length, int Write)
{
    FsIo_t q;
    FsIoResult_t r;

    q.Handle = Handle; q.Offset = Offset;
    q.Length = Length; q.WindowOffset = 0;
    r.Status = -1; r.Length = 0;

    if (SysCallTimed(Fs, Write ? FS_OP_WRITE : FS_OP_READ,
            &q, sizeof(q), &r, sizeof(r), 5000) < (int)sizeof(r)
        || r.Status != 0) {
        return -1;
    }
    return (int)r.Length;
}

static void Close(int Handle)
{
    FsHandle_t q; FsStatus_t r;
    q.Handle = Handle;
    SysCallTimed(Fs, FS_OP_CLOSE, &q, sizeof(q), &r, sizeof(r), 5000);
}

int ModuleMain(void)
{
    int Shm = -1, Attempt, i;

    SysPrint("[fsls] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    for (Attempt = 0; Attempt < 60; Attempt++) {
        if (SysLookup(FSSRV_NAME, &Fs, &Shm) == 0) { break; }
        SysSleep(50);
    }
    if (Fs < 0) {
        SysPrintLine("[fsls] no file service");
        SysExit(1);
    }

    Window = SysShmMap(Shm);
    if (Window == 0) {
        SysPrintLine("[fsls] could not map the window");
        SysExit(1);
    }

    /* list */
    {
        FsList_t q;
        FsListResult_t r;
        FsEntry_t *E = (FsEntry_t*)Window;
        int n;

        SetPath(q.Path, "/");
        r.Status = -1; r.Count = 0; r.Truncated = 0;

        n = SysCallTimed(Fs, FS_OP_LIST, &q, sizeof(q), &r, sizeof(r), 5000);
        if (n < (int)sizeof(r) || r.Status != 0) {
            SysPrintLine("[fsls] listing failed");
            SysExit(1);
        }
        if (r.Count < 0 || r.Count > FS_LIST_MAX) {
            SysPrintLine("[fsls] impossible entry count");
            SysExit(1);
        }

        SysPrintLine("[fsls] root directory:");
        for (i = 0; i < r.Count; i++) {
            SysPrint("    ");
            SysPrint(E[i].Name);
            SysPrint("  ");
            SysPrintNumber(E[i].Size);
            SysPrintLine(E[i].IsDirectory ? "  <DIR>" : " bytes");
        }
    }

    /* read an existing file */
    {
        unsigned Size = 0;
        int h = Open("/HELLO.TXT", FS_OPEN_READ, &Size);

        if (h < 0) {
            SysPrintLine("[fsls] could not open HELLO.TXT");
        }
        else {
            int n = Io(h, 0, Size, 0);
            if (n > 0) {
                SysPrint("[fsls] HELLO.TXT: ");
                SysWrite((const char*)Window, (unsigned)n);
            }
            Close(h);
        }
    }

    /* write a new file, then read it back through a fresh handle */
    {
        const char *Text = "written by TheTaaJ from ring 3\n";
        unsigned Length = SysStringLength(Text);
        unsigned i2;
        int h;

        h = Open("/TAAJ.TXT", FS_OPEN_WRITE | FS_OPEN_CREATE, 0);
        if (h < 0) {
            SysPrintLine("[fsls] could not create TAAJ.TXT");
        }
        else {
            char *W = (char*)Window;
            for (i2 = 0; i2 < Length; i2++) { W[i2] = Text[i2]; }

            if (Io(h, 0, Length, 1) == (int)Length) {
                SysPrint("[fsls] wrote ");
                SysPrintNumber(Length);
                SysPrintLine(" bytes to /TAAJ.TXT");
            }
            else {
                SysPrintLine("[fsls] write failed");
            }
            Close(h);

            {
                FsStatus_t r; 
                SysCallTimed(Fs, FS_OP_SYNC, 0, 0, &r, sizeof(r), 5000);
            }

            /* Read it back on a new handle - proves it reached the disk
             * and came back through a fresh directory lookup, not out of
             * anything still in memory. */
            {
                unsigned Size = 0;
                int h2 = Open("/TAAJ.TXT", FS_OPEN_READ, &Size);

                if (h2 < 0) {
                    SysPrintLine("[fsls] could not reopen TAAJ.TXT");
                }
                else {
                    int n = Io(h2, 0, Size, 0);
                    SysPrint("[fsls] read back ");
                    SysPrintNumber((unsigned)n);
                    SysPrint(" bytes: ");
                    if (n > 0) { SysWrite((const char*)Window, (unsigned)n); }
                    Close(h2);
                }
            }
        }
    }

    SysPrintLine("[fsls] done");
    SysExit(0);
    return 0;
}
