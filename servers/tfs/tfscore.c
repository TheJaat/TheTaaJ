/* TaajFS: copy-on-write. */

#include "tfscore.h"

static void Copy(void *D, const void *S, unsigned N)
{ unsigned char *d=(unsigned char*)D; const unsigned char *s=(const unsigned char*)S;
  unsigned i; for (i=0;i<N;i++) d[i]=s[i]; }

static void Zero(void *D, unsigned N)
{ unsigned char *d=(unsigned char*)D; unsigned i; for (i=0;i<N;i++) d[i]=0; }

static int NameEqual(const char *A, const char *B)
{
    int i;
    for (i = 0; i < TFS_NAME_MAX; i++) {
        if (A[i] != B[i]) { return 0; }
        if (A[i] == '\0') { return 1; }
    }
    return 1;
}

const char *TfsError(int Code)
{
    switch (Code) {
        case TFS_OK:            return "ok";
        case TFS_ERR_IO:        return "device error";
        case TFS_ERR_MAGIC:     return "not a TaajFS volume";
        case TFS_ERR_NOSUPER:   return "no valid superblock in either slot";
        case TFS_ERR_FULL:      return "no free blocks";
        case TFS_ERR_NOTFOUND:  return "not found";
        case TFS_ERR_EXISTS:    return "already exists";
        case TFS_ERR_NAME:      return "name too long";
        case TFS_ERR_TOOBIG:    return "file would exceed the maximum size";
        case TFS_ERR_READONLY:  return "volume is read only";
        default:                return "unknown";
    }
}

/* TfsChecksum
 * Additive over the superblock with the checksum field itself treated
 * as zero. Weak against deliberate tampering and entirely adequate
 * against the thing it exists for: a block that was half written when
 * the power went. */
static unsigned TfsChecksum(const TfsSuper_t *S)
{
    const unsigned char *b = (const unsigned char*)S;
    unsigned Sum = 0, i;
    unsigned Offset = (unsigned)((const unsigned char*)&S->Checksum
                               - (const unsigned char*)S);

    for (i = 0; i < sizeof(TfsSuper_t); i++) {
        if (i >= Offset && i < Offset + 4) { continue; }
        Sum += b[i];
        Sum = (Sum << 1) | (Sum >> 31);     /* rotate, so order matters */
    }
    return Sum;
}

/* ---- the free map ------------------------------------------------ */

static void MarkUsed(TfsVolume_t *V, unsigned Block)
{
    if (Block < TFS_MAX_BLOCKS && !(V->Used[Block / 8] & (1 << (Block % 8)))) {
        V->Used[Block / 8] |= (unsigned char)(1 << (Block % 8));
        V->UsedCount++;
    }
}

static int IsUsed(TfsVolume_t *V, unsigned Block)
{
    return (V->Used[Block / 8] & (1 << (Block % 8))) ? 1 : 0;
}

/* TfsAllocate
 * Hands out a block from the in-memory map.
 *
 * Nothing is written to disk here. A block handed out and then not
 * committed is simply free again at the next mount, because the free
 * map is derived from the committed tree and the uncommitted block is
 * not in it. That is the property that makes a failed write cost
 * nothing. */
static unsigned TfsAllocate(TfsVolume_t *V)
{
    unsigned i;

    for (i = TFS_FIRST_DATA; i < V->Super.TotalBlocks; i++) {
        if (!IsUsed(V, i)) {
            MarkUsed(V, i);
            return i;
        }
    }
    return 0;
}

/* TfsRebuildFreeMap
 * Walks the committed tree and marks everything reachable.
 *
 * This is the whole allocator. There is no on-disk bitmap to go stale,
 * no reference counts to leak, and no way for the allocator to disagree
 * with what the filesystem actually contains - the tree IS the
 * authority. */
static int TfsRebuildFreeMap(TfsVolume_t *V)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    unsigned DirBlock;
    unsigned Guard = 0;

    Zero(V->Used, sizeof(V->Used));
    V->UsedCount = 0;

    MarkUsed(V, TFS_SUPER_A);
    MarkUsed(V, TFS_SUPER_B);

    DirBlock = V->Super.RootDir;

    while (DirBlock != 0) {
        TfsDirBlock_t *Dir = (TfsDirBlock_t*)Block;
        unsigned i;

        if (++Guard > TFS_MAX_BLOCKS) { return TFS_ERR_IO; }
        if (V->Read(V->Context, DirBlock, Block) != 0) { return TFS_ERR_IO; }
        if (Dir->Magic != TFS_MAGIC) { return TFS_ERR_MAGIC; }

        MarkUsed(V, DirBlock);

        for (i = 0; i < Dir->Count && i < TFS_DIR_ENTRIES; i++) {
            unsigned char InodeBlock[TFS_BLOCK_SIZE];
            TfsInode_t *Inode = (TfsInode_t*)InodeBlock;
            unsigned b;

            if (Dir->Entries[i].Inode == 0) { continue; }
            MarkUsed(V, Dir->Entries[i].Inode);

            if (V->Read(V->Context, Dir->Entries[i].Inode, InodeBlock) != 0) {
                return TFS_ERR_IO;
            }
            if (Inode->Magic != TFS_MAGIC) { return TFS_ERR_MAGIC; }

            for (b = 0; b < Inode->BlockCount && b < TFS_INODE_BLOCKS; b++) {
                if (Inode->Blocks[b] != 0) { MarkUsed(V, Inode->Blocks[b]); }
            }
        }

        DirBlock = Dir->Next;
    }

    return TFS_OK;
}

/* ---- mount and format -------------------------------------------- */

int TfsFormat(TfsRead_t Read, TfsWrite_t Write, void *Context,
              unsigned TotalBlocks)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    TfsSuper_t *Super;
    TfsDirBlock_t *Dir;

    (void)Read;
    if (TotalBlocks < 8 || TotalBlocks > TFS_MAX_BLOCKS) {
        return TFS_ERR_FULL;
    }

    /* The root directory, empty. */
    Zero(Block, sizeof(Block));
    Dir = (TfsDirBlock_t*)Block;
    Dir->Magic = TFS_MAGIC;
    Dir->Count = 0;
    Dir->Next  = 0;
    if (Write(Context, TFS_FIRST_DATA, Block) != 0) { return TFS_ERR_IO; }

    Zero(Block, sizeof(Block));
    Super = (TfsSuper_t*)Block;
    Super->Magic       = TFS_MAGIC;
    Super->Version     = TFS_VERSION;
    Super->BlockSize   = TFS_BLOCK_SIZE;
    Super->TotalBlocks = TotalBlocks;
    Super->Generation  = 1;
    Super->RootDir     = TFS_FIRST_DATA;
    Super->Checksum    = TfsChecksum(Super);

    if (Write(Context, TFS_SUPER_A, Block) != 0) { return TFS_ERR_IO; }

    /* Slot B is deliberately left invalid rather than copied. A fresh
     * volume has exactly one valid superblock, and generation 1 is
     * unambiguously the newest. */
    Zero(Block, sizeof(Block));
    if (Write(Context, TFS_SUPER_B, Block) != 0) { return TFS_ERR_IO; }

    return TFS_OK;
}

int TfsMount(TfsVolume_t *V, TfsRead_t Read, TfsWrite_t Write, void *Context)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    TfsSuper_t *S = (TfsSuper_t*)Block;
    TfsSuper_t Best;
    int BestSlot = -1;
    int slot;

    Zero(V, sizeof(TfsVolume_t));
    V->Read = Read; V->Write = Write; V->Context = Context;
    Zero(&Best, sizeof(Best));

    /* Take the highest generation whose checksum is good. A half-written
     * superblock fails the checksum and is simply ignored, which is what
     * makes the commit atomic without any other machinery. */
    for (slot = TFS_SUPER_A; slot <= TFS_SUPER_B; slot++) {
        if (Read(Context, (unsigned)slot, Block) != 0) { continue; }
        if (S->Magic != TFS_MAGIC || S->Version != TFS_VERSION) { continue; }
        if (S->BlockSize != TFS_BLOCK_SIZE) { continue; }
        if (S->TotalBlocks == 0 || S->TotalBlocks > TFS_MAX_BLOCKS) { continue; }
        if (S->Checksum != TfsChecksum(S)) { continue; }

        if (BestSlot < 0 || S->Generation > Best.Generation) {
            Best = *S;
            BestSlot = slot;
        }
    }

    if (BestSlot < 0) { return TFS_ERR_NOSUPER; }

    V->Super     = Best;
    V->SuperSlot = BestSlot;
    V->Mounted   = 1;

    return TfsRebuildFreeMap(V);
}

/* TfsCommit
 * Publishes a new root with one block write.
 *
 * The new superblock goes in the OTHER slot. Until this write lands the
 * volume still reads as it did before; after it lands, every change is
 * visible at once. There is no state in between that a reader could
 * observe. */
static int TfsCommit(TfsVolume_t *V, unsigned NewRoot)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    TfsSuper_t *S = (TfsSuper_t*)Block;
    int Target = (V->SuperSlot == TFS_SUPER_A) ? TFS_SUPER_B : TFS_SUPER_A;

    Zero(Block, sizeof(Block));
    *S = V->Super;
    S->RootDir    = NewRoot;
    S->Generation = V->Super.Generation + 1;
    S->Checksum   = TfsChecksum(S);

    if (V->Write(V->Context, (unsigned)Target, Block) != 0) {
        return TFS_ERR_IO;
    }

    V->Super     = *S;
    V->SuperSlot = Target;

    /* The old tree's blocks are unreachable now, so recomputing the map
     * is what actually frees them. */
    return TfsRebuildFreeMap(V);
}

/* ---- reading ------------------------------------------------------ */

int TfsList(TfsVolume_t *V, TfsFile_t *Files, int Max)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    unsigned DirBlock;
    int Count = 0, Guard = 0;

    if (!V->Mounted) { return TFS_ERR_IO; }

    DirBlock = V->Super.RootDir;

    while (DirBlock != 0 && Count < Max) {
        TfsDirBlock_t *Dir = (TfsDirBlock_t*)Block;
        unsigned i;

        if (++Guard > TFS_MAX_BLOCKS) { return TFS_ERR_IO; }
        if (V->Read(V->Context, DirBlock, Block) != 0) { return TFS_ERR_IO; }

        for (i = 0; i < Dir->Count && i < TFS_DIR_ENTRIES && Count < Max; i++) {
            if (Dir->Entries[i].Inode == 0) { continue; }
            Copy(Files[Count].Name, Dir->Entries[i].Name, TFS_NAME_MAX);
            Files[Count].Inode = Dir->Entries[i].Inode;
            Files[Count].Size  = Dir->Entries[i].Size;
            Files[Count].Type  = Dir->Entries[i].Type;
            Count++;
        }

        DirBlock = Dir->Next;
    }

    return Count;
}

int TfsLookup(TfsVolume_t *V, const char *Name, TfsFile_t *File)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    unsigned DirBlock;
    int Guard = 0;

    if (!V->Mounted) { return TFS_ERR_IO; }
    while (*Name == '/') { Name++; }

    DirBlock = V->Super.RootDir;

    while (DirBlock != 0) {
        TfsDirBlock_t *Dir = (TfsDirBlock_t*)Block;
        unsigned i;

        if (++Guard > TFS_MAX_BLOCKS) { return TFS_ERR_IO; }
        if (V->Read(V->Context, DirBlock, Block) != 0) { return TFS_ERR_IO; }

        for (i = 0; i < Dir->Count && i < TFS_DIR_ENTRIES; i++) {
            if (Dir->Entries[i].Inode == 0) { continue; }
            if (NameEqual(Dir->Entries[i].Name, Name)) {
                Copy(File->Name, Dir->Entries[i].Name, TFS_NAME_MAX);
                File->Inode = Dir->Entries[i].Inode;
                File->Size  = Dir->Entries[i].Size;
                File->Type  = Dir->Entries[i].Type;
                return TFS_OK;
            }
        }

        DirBlock = Dir->Next;
    }

    return TFS_ERR_NOTFOUND;
}

int TfsRead(TfsVolume_t *V, const TfsFile_t *File, unsigned Offset,
            void *Buffer, unsigned Length)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    unsigned char InodeBlock[TFS_BLOCK_SIZE];
    TfsInode_t *Inode = (TfsInode_t*)InodeBlock;
    unsigned char *Out = (unsigned char*)Buffer;
    unsigned Done = 0;

    if (!V->Mounted) { return TFS_ERR_IO; }
    if (Offset >= File->Size) { return 0; }
    if ((Offset + Length) > File->Size) { Length = File->Size - Offset; }

    if (V->Read(V->Context, File->Inode, InodeBlock) != 0) { return TFS_ERR_IO; }
    if (Inode->Magic != TFS_MAGIC) { return TFS_ERR_MAGIC; }

    while (Done < Length) {
        unsigned At = Offset + Done;
        unsigned Index = At / TFS_BLOCK_SIZE;
        unsigned In = At % TFS_BLOCK_SIZE;
        unsigned Chunk = TFS_BLOCK_SIZE - In;

        if (Index >= Inode->BlockCount) { break; }
        if (Chunk > (Length - Done)) { Chunk = Length - Done; }

        if (V->Read(V->Context, Inode->Blocks[Index], Block) != 0) {
            return (int)Done;
        }
        Copy(Out + Done, Block + In, Chunk);
        Done += Chunk;
    }

    return (int)Done;
}

unsigned TfsFreeBlocks(TfsVolume_t *V)
{
    return V->Super.TotalBlocks - V->UsedCount;
}

/* ---- writing ------------------------------------------------------ */

/* TfsRewriteDirectory
 * Writes a complete new copy of the directory chain, applying one
 * change, and returns the new head block.
 *
 * Rewriting the whole chain rather than the one block that changed is
 * wasteful and correct: a directory is at most a few blocks here, and
 * the alternative is tracking which block held which entry so the rest
 * can be shared. The simpler version is the one that cannot be subtly
 * wrong. */
static int TfsRewriteDirectory(TfsVolume_t *V, const char *Name,
                               unsigned Inode, unsigned Size, unsigned Type,
                               int Remove, unsigned *NewHead)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    TfsDirEntry_t All[TFS_DIR_ENTRIES * 8];
    unsigned Total = 0;
    unsigned DirBlock;
    int Guard = 0, Replaced = 0;
    unsigned i, Head = 0, Previous = 0;

    /* Gather every surviving entry. */
    DirBlock = V->Super.RootDir;
    while (DirBlock != 0) {
        TfsDirBlock_t *Dir = (TfsDirBlock_t*)Block;

        if (++Guard > 8) { return TFS_ERR_FULL; }
        if (V->Read(V->Context, DirBlock, Block) != 0) { return TFS_ERR_IO; }

        for (i = 0; i < Dir->Count && i < TFS_DIR_ENTRIES; i++) {
            if (Dir->Entries[i].Inode == 0) { continue; }

            if (NameEqual(Dir->Entries[i].Name, Name)) {
                if (Remove) { Replaced = 1; continue; }
                All[Total] = Dir->Entries[i];
                All[Total].Inode = Inode;
                All[Total].Size  = Size;
                All[Total].Type  = Type;
                Total++;
                Replaced = 1;
                continue;
            }

            if (Total >= (TFS_DIR_ENTRIES * 8)) { return TFS_ERR_FULL; }
            All[Total++] = Dir->Entries[i];
        }
        DirBlock = Dir->Next;
    }

    if (!Replaced) {
        if (Remove) { return TFS_ERR_NOTFOUND; }
        if (Total >= (TFS_DIR_ENTRIES * 8)) { return TFS_ERR_FULL; }
        Zero(&All[Total], sizeof(TfsDirEntry_t));
        Copy(All[Total].Name, Name, TFS_NAME_MAX);
        All[Total].Inode = Inode;
        All[Total].Size  = Size;
        All[Total].Type  = Type;
        Total++;
    }

    /* Write the new chain. Always at least one block, so an empty
     * directory still has a root to point at. */
    i = 0;
    do {
        TfsDirBlock_t *Dir = (TfsDirBlock_t*)Block;
        unsigned New = TfsAllocate(V);
        unsigned n = 0;

        if (New == 0) { return TFS_ERR_FULL; }

        Zero(Block, sizeof(Block));
        Dir->Magic = TFS_MAGIC;
        Dir->Next  = 0;

        while (n < TFS_DIR_ENTRIES && i < Total) {
            Dir->Entries[n++] = All[i++];
        }
        Dir->Count = n;

        if (V->Write(V->Context, New, Block) != 0) { return TFS_ERR_IO; }

        if (Head == 0) {
            Head = New;
        }
        else {
            /* Link the previous block to this one. Re-reading and
             * rewriting it is safe: it is a block we allocated during
             * this uncommitted change, not a live one. */
            unsigned char Prev[TFS_BLOCK_SIZE];
            TfsDirBlock_t *P = (TfsDirBlock_t*)Prev;

            if (V->Read(V->Context, Previous, Prev) != 0) { return TFS_ERR_IO; }
            P->Next = New;
            if (V->Write(V->Context, Previous, Prev) != 0) { return TFS_ERR_IO; }
        }
        Previous = New;
    } while (i < Total);

    *NewHead = Head;
    return TFS_OK;
}

int TfsCreate(TfsVolume_t *V, const char *Name)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    TfsInode_t *Inode = (TfsInode_t*)Block;
    TfsFile_t Existing;
    unsigned InodeBlock, NewRoot;
    int Status;
    unsigned i;

    if (!V->Mounted)   { return TFS_ERR_IO; }
    if (V->Write == 0) { return TFS_ERR_READONLY; }

    while (*Name == '/') { Name++; }
    for (i = 0; Name[i] != '\0'; i++) {
        if (i >= TFS_NAME_MAX - 1) { return TFS_ERR_NAME; }
    }
    if (i == 0) { return TFS_ERR_NAME; }

    if (TfsLookup(V, Name, &Existing) == TFS_OK) { return TFS_ERR_EXISTS; }

    InodeBlock = TfsAllocate(V);
    if (InodeBlock == 0) { return TFS_ERR_FULL; }

    Zero(Block, sizeof(Block));
    Inode->Magic = TFS_MAGIC;
    Inode->Size  = 0;
    Inode->Type  = TFS_TYPE_FILE;
    Inode->BlockCount = 0;
    if (V->Write(V->Context, InodeBlock, Block) != 0) { return TFS_ERR_IO; }

    Status = TfsRewriteDirectory(V, Name, InodeBlock, 0, TFS_TYPE_FILE,
                                 0, &NewRoot);
    if (Status != TFS_OK) { return Status; }

    return TfsCommit(V, NewRoot);
}

int TfsWriteFile(TfsVolume_t *V, const char *Name, unsigned Offset,
                 const void *Buffer, unsigned Length)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    unsigned char InodeBlock[TFS_BLOCK_SIZE];
    TfsInode_t *Inode = (TfsInode_t*)InodeBlock;
    const unsigned char *In = (const unsigned char*)Buffer;
    TfsFile_t File;
    unsigned NewInode, NewRoot, NewSize, Done = 0;
    int Status;

    if (!V->Mounted)   { return TFS_ERR_IO; }
    if (V->Write == 0) { return TFS_ERR_READONLY; }
    if ((Offset + Length) > TFS_MAX_FILE_SIZE) { return TFS_ERR_TOOBIG; }

    Status = TfsLookup(V, Name, &File);
    if (Status != TFS_OK) { return Status; }

    if (V->Read(V->Context, File.Inode, InodeBlock) != 0) { return TFS_ERR_IO; }
    if (Inode->Magic != TFS_MAGIC) { return TFS_ERR_MAGIC; }

    NewSize = File.Size;
    if ((Offset + Length) > NewSize) { NewSize = Offset + Length; }

    /* Copy each touched data block to a new home before changing it.
     * The old block stays exactly as it was, which is what the previous
     * superblock still points at. */
    while (Done < Length) {
        unsigned At = Offset + Done;
        unsigned Index = At / TFS_BLOCK_SIZE;
        unsigned InBlock = At % TFS_BLOCK_SIZE;
        unsigned Chunk = TFS_BLOCK_SIZE - InBlock;
        unsigned New;

        if (Index >= TFS_INODE_BLOCKS) { return TFS_ERR_TOOBIG; }
        if (Chunk > (Length - Done)) { Chunk = Length - Done; }

        Zero(Block, sizeof(Block));
        if (Index < Inode->BlockCount && Inode->Blocks[Index] != 0) {
            if (V->Read(V->Context, Inode->Blocks[Index], Block) != 0) {
                return TFS_ERR_IO;
            }
        }

        Copy(Block + InBlock, In + Done, Chunk);

        New = TfsAllocate(V);
        if (New == 0) { return TFS_ERR_FULL; }
        if (V->Write(V->Context, New, Block) != 0) { return TFS_ERR_IO; }

        Inode->Blocks[Index] = New;
        if (Index >= Inode->BlockCount) {
            unsigned b;
            /* A write past the end can leave a gap. Fill it with real
             * zeroed blocks rather than leaving holes: a hole read as a
             * stale block number would return another file's data. */
            for (b = Inode->BlockCount; b < Index; b++) {
                unsigned Fill = TfsAllocate(V);
                unsigned char ZeroBlock[TFS_BLOCK_SIZE];

                if (Fill == 0) { return TFS_ERR_FULL; }
                Zero(ZeroBlock, sizeof(ZeroBlock));
                if (V->Write(V->Context, Fill, ZeroBlock) != 0) {
                    return TFS_ERR_IO;
                }
                Inode->Blocks[b] = Fill;
            }
            Inode->BlockCount = Index + 1;
        }

        Done += Chunk;
    }

    Inode->Size = NewSize;

    NewInode = TfsAllocate(V);
    if (NewInode == 0) { return TFS_ERR_FULL; }
    if (V->Write(V->Context, NewInode, InodeBlock) != 0) { return TFS_ERR_IO; }

    Status = TfsRewriteDirectory(V, Name, NewInode, NewSize, TFS_TYPE_FILE,
                                 0, &NewRoot);
    if (Status != TFS_OK) { return Status; }

    Status = TfsCommit(V, NewRoot);
    if (Status != TFS_OK) { return Status; }

    return (int)Done;
}

int TfsDelete(TfsVolume_t *V, const char *Name)
{
    unsigned NewRoot;
    TfsFile_t File;
    int Status;

    if (!V->Mounted)   { return TFS_ERR_IO; }
    if (V->Write == 0) { return TFS_ERR_READONLY; }

    while (*Name == '/') { Name++; }

    Status = TfsLookup(V, Name, &File);
    if (Status != TFS_OK) { return Status; }

    /* Nothing is erased. The entry simply does not appear in the new
     * directory, so the inode and its data become unreachable - and
     * therefore free - the moment the commit lands. */
    Status = TfsRewriteDirectory(V, Name, 0, 0, 0, 1, &NewRoot);
    if (Status != TFS_OK) { return Status; }

    return TfsCommit(V, NewRoot);
}
