/* sh - an interactive shell, in ring 3.
 *
 * It owns no hardware and talks to no device. Keystrokes arrive on a
 * pipe published by the PS/2 driver; files come from the file service;
 * output goes through the write syscall. Three processes it has never
 * heard of, found by name.
 *
 * This is the first program in the system that a person uses directly
 * rather than a demonstration that runs and exits. */

#include <os/syscall.h>
#include <os/registry.h>
#include <os/fssrv.h>

#define LINE_MAX    96
#define ARG_MAX     64

static int Keyboard = -1;
static int Fs = -1;
static unsigned Window = 0;

/* ---- tiny string helpers; there is no libc ---- */

static unsigned Len(const char *s)
{ unsigned n = 0; while (s[n]) n++; return n; }

static int Equal(const char *a, const char *b)
{
    unsigned i;
    for (i = 0; ; i++) {
        if (a[i] != b[i]) return 0;
        if (a[i] == '\0') return 1;
    }
}

static void Copy(char *d, const char *s, unsigned Max)
{
    unsigned i;
    for (i = 0; i < Max - 1 && s[i]; i++) d[i] = s[i];
    for (; i < Max; i++) d[i] = '\0';
}

static unsigned ParseUInt(const char *s, int *Ok)
{
    unsigned v = 0; int digits = 0;
    while (*s == ' ') s++;
    while (*s >= '0' && *s <= '9') { v = v * 10 + (unsigned)(*s - '0'); s++; digits++; }
    if (Ok) *Ok = digits ? 1 : 0;
    return v;
}

/* ---- file service wrappers ---- */

static void SetPath(char *Dst, const char *Src)
{
    unsigned i;
    for (i = 0; i < FS_PATH_MAX - 1 && Src[i]; i++) Dst[i] = Src[i];
    for (; i < FS_PATH_MAX; i++) Dst[i] = '\0';
}

static int Open(const char *Path, unsigned Flags, unsigned *Size)
{
    FsOpen_t q; FsHandleResult_t r;
    SetPath(q.Path, Path); q.Flags = Flags;
    r.Status = -1; r.Handle = -1; r.Size = 0;
    if (SysCallTimed(Fs, FS_OP_OPEN, &q, sizeof(q), &r, sizeof(r), 5000)
        < (int)sizeof(r) || r.Status != 0) return -1;
    if (Size) *Size = r.Size;
    return r.Handle;
}

static int Io(int Handle, unsigned Offset, unsigned Length, int Write)
{
    FsIo_t q; FsIoResult_t r;
    q.Handle = Handle; q.Offset = Offset;
    q.Length = Length; q.WindowOffset = 0;
    r.Status = -1; r.Length = 0;
    if (SysCallTimed(Fs, Write ? FS_OP_WRITE : FS_OP_READ,
            &q, sizeof(q), &r, sizeof(r), 5000) < (int)sizeof(r)
        || r.Status != 0) return -1;
    return (int)r.Length;
}

static void Close(int h)
{ FsHandle_t q; FsStatus_t r; q.Handle = h;
  SysCallTimed(Fs, FS_OP_CLOSE, &q, sizeof(q), &r, sizeof(r), 5000); }

static int Simple(unsigned Opcode, const char *Path)
{
    FsPath_t q; FsStatus_t r;
    SetPath(q.Path, Path); r.Status = -1;
    if (SysCallTimed(Fs, Opcode, &q, sizeof(q), &r, sizeof(r), 5000)
        < (int)sizeof(r)) return -1;
    return r.Status;
}

static void Sync(void)
{ FsStatus_t r; SysCallTimed(Fs, FS_OP_SYNC, 0, 0, &r, sizeof(r), 5000); }

/* ---- commands ---- */

static void CmdHelp(void)
{
    SysPrintLine("commands:");
    SysPrintLine("  ls [path]           list a directory");
    SysPrintLine("  cat <path>          print a file");
    SysPrintLine("  write <path> <text> create or overwrite a file");
    SysPrintLine("  append <path> <text>");
    SysPrintLine("  rm <path>           delete a file or empty directory");
    SysPrintLine("  mkdir <path>        create a directory");
    SysPrintLine("  truncate <path> <n>");
    SysPrintLine("  stat <path>");
    SysPrintLine("  sync                flush the filesystem");
    SysPrintLine("  help");
}

static void CmdLs(const char *Path)
{
    FsList_t q; FsListResult_t r;
    FsEntry_t *E = (FsEntry_t*)Window;
    int n, i;

    SetPath(q.Path, (Path && *Path) ? Path : "/");
    r.Status = -1; r.Count = 0; r.Truncated = 0;

    n = SysCallTimed(Fs, FS_OP_LIST, &q, sizeof(q), &r, sizeof(r), 5000);
    if (n < (int)sizeof(r) || r.Status != 0) {
        SysPrintLine("ls: cannot list that");
        return;
    }
    if (r.Count < 0 || r.Count > FS_LIST_MAX) {
        SysPrintLine("ls: the reply makes no sense");
        return;
    }

    /* The entries are in the shared window; the reply only said how
     * many. That is what lifts the old six-entry ceiling. */
    for (i = 0; i < r.Count; i++) {
        SysPrint("  ");
        SysPrint(E[i].Name);
        SysPrint("  ");
        SysPrintNumber(E[i].Size);
        SysPrintLine(E[i].IsDirectory ? "  <DIR>" : "");
    }
    SysPrint("  ");
    SysPrintNumber((unsigned)r.Count);
    SysPrintLine(r.Truncated ? " entries (more exist)" : " entries");
}

static void CmdCat(const char *Path)
{
    unsigned Size = 0, Done = 0;
    int h = Open(Path, FS_OPEN_READ, &Size);

    if (h < 0) { SysPrintLine("cat: cannot open"); return; }

    /* A file can be larger than the shared window, so read it in
     * window-sized pieces rather than assuming it fits. */
    while (Done < Size) {
        unsigned Want = Size - Done;
        int n;

        if (Want > FS_WINDOW_BYTES) Want = FS_WINDOW_BYTES;
        n = Io(h, Done, Want, 0);
        if (n <= 0) break;
        SysWrite((const char*)Window, (unsigned)n);
        Done += (unsigned)n;
    }
    Close(h);
}

static void CmdWrite(const char *Path, const char *Text, int Append)
{
    unsigned Size = 0, Length = Len(Text);
    unsigned i;
    char *W = (char*)Window;
    int h;

    if (Length == 0) { SysPrintLine("write: nothing to write"); return; }
    if (Length > FS_WINDOW_BYTES) Length = FS_WINDOW_BYTES;

    if (!Append) {
        /* Overwrite means overwrite. Without truncating first, writing
         * a short string over a long file leaves the old tail behind -
         * which looks like corruption rather than a stale remainder. */
        FsTruncate_t t; FsStatus_t r;
        SetPath(t.Path, Path); t.Size = 0;
        SysCallTimed(Fs, FS_OP_TRUNCATE, &t, sizeof(t), &r, sizeof(r), 5000);
    }

    h = Open(Path, FS_OPEN_WRITE | FS_OPEN_CREATE, &Size);
    if (h < 0) { SysPrintLine("write: cannot open"); return; }

    for (i = 0; i < Length; i++) W[i] = Text[i];
    W[Length] = '\n';
    Length++;

    if (Io(h, Append ? Size : 0, Length, 1) != (int)Length) {
        SysPrintLine("write: failed");
    }
    Close(h);
    Sync();
}

static void CmdStat(const char *Path)
{
    FsPath_t q; FsStatResult_t r;

    SetPath(q.Path, Path); r.Status = -1;
    if (SysCallTimed(Fs, FS_OP_STAT, &q, sizeof(q), &r, sizeof(r), 5000)
        < (int)sizeof(r) || r.Status != 0) {
        SysPrintLine("stat: not found");
        return;
    }
    SysPrint("  ");
    SysPrint(r.Entry.Name);
    SysPrint("  ");
    SysPrintNumber(r.Entry.Size);
    SysPrintLine(r.Entry.IsDirectory ? " bytes  <DIR>" : " bytes");
}

/* ---- line handling ---- */

static void Execute(char *Line)
{
    char *Argument;
    int i;

    while (*Line == ' ') Line++;
    if (*Line == '\0') return;

    /* Split on the first space. */
    for (i = 0; Line[i] && Line[i] != ' '; i++) { }
    if (Line[i] == ' ') { Line[i] = '\0'; Argument = Line + i + 1; }
    else                { Argument = Line + i; }
    while (*Argument == ' ') Argument++;

    if (Equal(Line, "help")) { CmdHelp(); return; }
    if (Equal(Line, "ls"))   { CmdLs(Argument); return; }
    if (Equal(Line, "cat"))  { CmdCat(Argument); return; }
    if (Equal(Line, "stat")) { CmdStat(Argument); return; }
    if (Equal(Line, "sync")) { Sync(); SysPrintLine("synced"); return; }

    if (Equal(Line, "mkdir")) {
        if (Simple(FS_OP_MKDIR, Argument) == 0) SysPrintLine("created");
        else SysPrintLine("mkdir: failed");
        Sync();
        return;
    }

    if (Equal(Line, "rm")) {
        if (Simple(FS_OP_DELETE, Argument) == 0) SysPrintLine("removed");
        else SysPrintLine("rm: failed");
        Sync();
        return;
    }

    if (Equal(Line, "write") || Equal(Line, "append")) {
        char Path[FS_PATH_MAX];
        int j;
        for (j = 0; Argument[j] && Argument[j] != ' '; j++) { }
        if (Argument[j] != ' ') { SysPrintLine("usage: write <path> <text>"); return; }
        Argument[j] = '\0';
        Copy(Path, Argument, FS_PATH_MAX);
        CmdWrite(Path, Argument + j + 1, Equal(Line, "append"));
        return;
    }

    if (Equal(Line, "truncate")) {
        char Path[FS_PATH_MAX];
        FsTruncate_t t; FsStatus_t r;
        int j, Ok = 0;
        for (j = 0; Argument[j] && Argument[j] != ' '; j++) { }
        if (Argument[j] != ' ') { SysPrintLine("usage: truncate <path> <n>"); return; }
        Argument[j] = '\0';
        Copy(Path, Argument, FS_PATH_MAX);
        t.Size = ParseUInt(Argument + j + 1, &Ok);
        if (!Ok) { SysPrintLine("truncate: expected a number"); return; }
        SetPath(t.Path, Path);
        r.Status = -1;
        SysCallTimed(Fs, FS_OP_TRUNCATE, &t, sizeof(t), &r, sizeof(r), 5000);
        SysPrintLine(r.Status == 0 ? "truncated" : "truncate: failed");
        Sync();
        return;
    }

    SysPrint("unknown command: ");
    SysPrintLine(Line);
}

int ModuleMain(void)
{
    char Line[LINE_MAX];
    unsigned Length = 0;
    int Shm = -1, Attempt;

    SysPrint("[sh] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    for (Attempt = 0; Attempt < 60; Attempt++) {
        if (Keyboard < 0) SysLookup("keyboard", &Keyboard, 0);
        if (Fs < 0)       SysLookup(FSSRV_NAME, &Fs, &Shm);
        if (Keyboard >= 0 && Fs >= 0) break;
        SysSleep(50);
    }
    if (Keyboard < 0) { SysPrintLine("[sh] no keyboard service"); SysExit(1); }
    if (Fs < 0)       { SysPrintLine("[sh] no file service");     SysExit(1); }

    Window = SysShmMap(Shm);
    if (Window == 0) { SysPrintLine("[sh] no window"); SysExit(1); }

    SysPrintLine("");
    SysPrintLine("TheTaaJ shell, ring 3. Type 'help'.");
    SysPrint("$ ");

    for (;;) {
        char c;

        if (SysPipeRead(Keyboard, &c, 1) != 1) {
            continue;
        }

        if (c == '\n') {
            SysPrint("\n");
            Line[Length] = '\0';
            Execute(Line);
            Length = 0;
            SysPrint("$ ");
            continue;
        }
        if (c == '\b') {
            /* Only erase when there is something to erase, or the
             * cursor walks back over the prompt. */
            if (Length > 0) { Length--; SysWrite("\b", 1); }
            continue;
        }
        if (c < 0x20 || c > 0x7E) {
            continue;
        }
        if (Length >= (LINE_MAX - 1)) {
            continue;       /* refuse rather than truncate silently */
        }

        Line[Length++] = c;
        SysWrite(&c, 1);
    }

    return 0;
}
