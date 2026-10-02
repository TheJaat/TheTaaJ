/* fsls - lists the disk and prints a file. A client of a client.
 *
 *   fsls -> fat32 -> ata -> hardware
 *
 * Four processes, four address spaces, and the only thing that crosses
 * between them is messages and a shared page. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/fssrv.h>

/* Mark
 * One character, written immediately, before and after each step.
 *
 * A register dump tells you where a fault happened; it does not tell you
 * what the program had already managed to do. These letters do, and the
 * last one printed brackets the failure to a single statement. */
static void Mark(const char *Tag)
{
    SysPrint("<");
    SysPrint(Tag);
    SysPrint(">");
}

int ModuleMain(void)
{
    int Fs = -1, Shm = -1, Attempt, i, n;
    unsigned Window;

    SysPrint("[fsls] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    Mark("lookup");
    for (Attempt = 0; Attempt < 60; Attempt++) {
        if (SysLookup(FSSRV_NAME, &Fs, &Shm) == 0) {
            break;
        }
        SysSleep(50);
    }
    if (Fs < 0) {
        SysPrintLine("[fsls] no file service");
        SysExit(1);
    }
    Mark("got-fs");
    SysPrint(" fs="); SysPrintNumber((unsigned)Fs);
    SysPrint(" shm="); SysPrintNumber((unsigned)Shm); SysPrint(" ");

    Window = SysShmMap(Shm);
    Mark("mapped");
    SysPrintHex(Window);
    if (Window == 0) {
        /* A mapping can fail - the region may belong to a process that
         * has since exited. Carrying on would use zero as a base
         * address, and the fault lands one whole struct size away from
         * the mistake. */
        SysPrintLine("[fsls] could not map the file service window");
        SysExit(1);
    }

    {
        FsList_t q;
        FsListResult_t r;

        q.Cluster = 0;
        r.Status = -1; r.Count = 0;

        /* Check the CALL, not just the reply. A call to a dead service
         * returns negative and never touches the reply buffer, so
         * reading r at all would be reading whatever was on the stack. */
        Mark("list-call");
        n = SysCallTimed(Fs, FS_OP_LIST, &q, sizeof(q), &r, sizeof(r), 5000);
        Mark("list-returned");
        SysPrintNumber((unsigned)n);
        if (n < (int)sizeof(r)) {
            SysPrint("[fsls] the file service did not answer (");
            SysPrintNumber((unsigned)n);
            SysPrintLine(")");
            SysExit(1);
        }
        if (r.Status != 0) {
            SysPrintLine("[fsls] listing failed");
            SysExit(1);
        }
        if (r.Count < 0 || r.Count > FS_LIST_MAX) {
            SysPrintLine("[fsls] the reply claims an impossible entry count");
            SysExit(1);
        }

        Mark("checks-passed");
        SysPrint(" count="); SysPrintNumber((unsigned)r.Count); SysPrint("\n");

        SysPrintLine("[fsls] root directory:");
        for (i = 0; i < r.Count; i++) {
            Mark("entry");
            SysPrintNumber((unsigned)i);
            SysPrint("    ");
            SysPrint(r.Entries[i].Name);
            SysPrint("  ");
            SysPrintNumber(r.Entries[i].Size);
            SysPrintLine(r.Entries[i].IsDirectory ? "  <DIR>" : " bytes");
        }
    }

    {
        FsRead_t q;
        FsReadResult_t r;
        unsigned j;

        for (j = 0; j < FS_NAME_MAX; j++) { q.Name[j] = 0; }
        q.Name[0]='H'; q.Name[1]='E'; q.Name[2]='L'; q.Name[3]='L';
        q.Name[4]='O'; q.Name[5]='.'; q.Name[6]='T'; q.Name[7]='X';
        q.Name[8]='T';
        q.Offset = 0;
        q.Length = 256;
        Mark("read-call");

        r.Status = -1; r.Length = 0;
        n = SysCallTimed(Fs, FS_OP_READ, &q, sizeof(q), &r, sizeof(r), 5000);

        Mark("read-returned");
        SysPrintNumber((unsigned)n);

        if (n >= (int)sizeof(r) && r.Status == 0
            && r.Length <= FS_WINDOW_BYTES) {
            Mark("about-to-write");
            char *Data = (char*)Window;
            SysPrint("[fsls] HELLO.TXT (");
            SysPrintNumber(r.Length);
            SysPrintLine(" bytes):");
            SysPrint("    ");
            SysWrite(Data, r.Length);
        }
        else {
            SysPrintLine("[fsls] could not read HELLO.TXT");
        }
    }

    SysPrintLine("[fsls] done");
    SysExit(0);
    return 0;
}
