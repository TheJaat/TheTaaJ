/* FAT32: read, write, cache, paths. */

#include "fat32core.h"

static void CoreCopy(void *Dst, const void *Src, unsigned Length)
{
    unsigned char *d = (unsigned char*)Dst;
    const unsigned char *s = (const unsigned char*)Src;
    unsigned i;
    for (i = 0; i < Length; i++) { d[i] = s[i]; }
}

static void CoreSet(void *Dst, int Value, unsigned Length)
{
    unsigned char *d = (unsigned char*)Dst;
    unsigned i;
    for (i = 0; i < Length; i++) { d[i] = (unsigned char)Value; }
}

static unsigned Fat32ClusterToSector(Fat32Volume_t *V, unsigned Cluster)
{
    return V->FirstDataSector + ((Cluster - 2) * V->SectorsPerCluster);
}

const char *Fat32Error(int Code)
{
    switch (Code) {
        case FAT32_OK:            return "ok";
        case FAT32_ERR_READ:      return "device read failed";
        case FAT32_ERR_SIGNATURE: return "no 0x55AA in sector 0";
        case FAT32_ERR_GEOMETRY:  return "impossible sector or cluster size";
        case FAT32_ERR_NOT_FAT32: return "FAT12/16 layout, not FAT32";
        case FAT32_ERR_TOO_SMALL: return "fewer than 65525 clusters";
        case FAT32_ERR_WRITE:     return "device write failed";
        case FAT32_ERR_FULL:      return "no free clusters";
        case FAT32_ERR_READONLY:  return "volume is read only";
        case FAT32_ERR_NOTFOUND:  return "not found";
        case FAT32_ERR_EXISTS:    return "already exists";
        case FAT32_ERR_NAME:      return "name is not valid 8.3";
        default:                  return "unknown";
    }
}

/* ---------------------------------------------------------------- */
/* the FAT, through a one-sector cache                               */
/* ---------------------------------------------------------------- */

/* Fat32FlushCache
 * Writes the cached sector to EVERY FAT copy.
 *
 * There are normally two, and they must agree. Updating only the first
 * leaves a volume that this driver reads back correctly and that every
 * checker calls corrupt - the kind of damage that is invisible until
 * something else looks at the disk. */
static int Fat32FlushCache(Fat32Volume_t *V)
{
    unsigned i;

    if (!V->FatCacheDirty || V->FatCacheSector == 0) {
        return FAT32_OK;
    }
    if (V->Write == 0) {
        return FAT32_ERR_READONLY;
    }

    for (i = 0; i < V->FatCount; i++) {
        unsigned Sector = V->FatCacheSector + (i * V->SectorsPerFat);

        if (V->Write(V->Context, Sector, 1, V->FatCache) != 0) {
            return FAT32_ERR_WRITE;
        }
    }

    V->FatCacheDirty = 0;
    return FAT32_OK;
}

/* Fat32UpdateFsInfo
 * Subtracts what we allocated from the recorded free count, and points
 * the next-free hint at where the allocator stopped.
 *
 * A count of 0xFFFFFFFF means "unknown", which is legal and must be
 * left alone - replacing it with a number we have not actually verified
 * would be worse than admitting ignorance. */
static int Fat32UpdateFsInfo(Fat32Volume_t *V)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    unsigned Free;

    if (V->ClustersAllocated == 0 || V->FsInfoSector == 0 || V->Write == 0) {
        return FAT32_OK;
    }
    if (V->Read(V->Context, V->FsInfoSector, 1, Sector) != 0) {
        return FAT32_ERR_READ;
    }

    /* Two signatures guard the structure; without both, this is not an
     * FSInfo sector and writing to it would corrupt something else. */
    if (!(Sector[0] == 0x52 && Sector[1] == 0x52
          && Sector[2] == 0x61 && Sector[3] == 0x41)) {
        return FAT32_OK;
    }
    if (!(Sector[484] == 0x72 && Sector[485] == 0x72
          && Sector[486] == 0x41 && Sector[487] == 0x61)) {
        return FAT32_OK;
    }

    Free = (unsigned)Sector[488] | ((unsigned)Sector[489] << 8)
         | ((unsigned)Sector[490] << 16) | ((unsigned)Sector[491] << 24);

    if (Free != 0xFFFFFFFFu) {
        Free = (Free >= V->ClustersAllocated) ? (Free - V->ClustersAllocated) : 0;
        Sector[488] = (unsigned char)(Free & 0xFF);
        Sector[489] = (unsigned char)((Free >> 8) & 0xFF);
        Sector[490] = (unsigned char)((Free >> 16) & 0xFF);
        Sector[491] = (unsigned char)((Free >> 24) & 0xFF);
    }

    if (V->Write(V->Context, V->FsInfoSector, 1, Sector) != 0) {
        return FAT32_ERR_WRITE;
    }

    V->ClustersAllocated = 0;
    return FAT32_OK;
}

int Fat32Flush(Fat32Volume_t *V)
{
    int Status = Fat32FlushCache(V);

    if (Status != FAT32_OK) {
        return Status;
    }
    return Fat32UpdateFsInfo(V);
}

/* Fat32LoadFatSector
 * Brings the FAT sector holding <Cluster> into the cache. Flushes first
 * if a different sector is cached and dirty. */
static int Fat32LoadFatSector(Fat32Volume_t *V, unsigned Cluster,
                              unsigned *EntryOffset)
{
    unsigned FatOffset = Cluster * 4;
    unsigned Sector = V->FirstFatSector + (FatOffset / V->BytesPerSector);

    *EntryOffset = FatOffset % V->BytesPerSector;

    if (V->FatCacheSector == Sector) {
        V->CacheHits++;
        return FAT32_OK;
    }

    if (Fat32FlushCache(V) != FAT32_OK) {
        return FAT32_ERR_WRITE;
    }

    if (V->Read(V->Context, Sector, 1, V->FatCache) != 0) {
        V->FatCacheSector = 0;
        return FAT32_ERR_READ;
    }

    V->FatCacheSector = Sector;
    V->CacheMisses++;
    return FAT32_OK;
}

unsigned Fat32NextCluster(Fat32Volume_t *V, unsigned Cluster)
{
    unsigned Offset, Next;

    if (!V->Mounted || Cluster < 2) {
        return 0;
    }
    if (Fat32LoadFatSector(V, Cluster, &Offset) != FAT32_OK) {
        return 0;
    }

    Next = (unsigned)V->FatCache[Offset]
         | ((unsigned)V->FatCache[Offset + 1] << 8)
         | ((unsigned)V->FatCache[Offset + 2] << 16)
         | ((unsigned)V->FatCache[Offset + 3] << 24);

    /* The top four bits are reserved. Treating the entry as a full
     * 32-bit value makes every end-of-chain marker look like a wild
     * cluster number. */
    Next &= FAT32_CLUSTER_MASK;

    if (Next >= FAT32_CLUSTER_BAD) {
        return 0;
    }
    return Next;
}

/* Fat32SetCluster
 * Writes one FAT entry, preserving the reserved high bits. */
static int Fat32SetCluster(Fat32Volume_t *V, unsigned Cluster, unsigned Value)
{
    unsigned Offset, Existing;
    int Status;

    if (V->Write == 0) {
        return FAT32_ERR_READONLY;
    }
    Status = Fat32LoadFatSector(V, Cluster, &Offset);
    if (Status != FAT32_OK) {
        return Status;
    }

    /* Keep the top four bits as they were. They are reserved, and
     * clearing them is a change to the volume that nothing asked for. */
    Existing = ((unsigned)V->FatCache[Offset + 3] << 24) & 0xF0000000u;
    Value = (Value & FAT32_CLUSTER_MASK) | Existing;

    V->FatCache[Offset]     = (unsigned char)(Value & 0xFF);
    V->FatCache[Offset + 1] = (unsigned char)((Value >> 8) & 0xFF);
    V->FatCache[Offset + 2] = (unsigned char)((Value >> 16) & 0xFF);
    V->FatCache[Offset + 3] = (unsigned char)((Value >> 24) & 0xFF);
    V->FatCacheDirty = 1;

    return FAT32_OK;
}

/* Fat32AllocateCluster
 * Finds a free cluster, marks it end-of-chain, and returns it.
 *
 * Marking it immediately matters: a cluster left as free after being
 * handed out can be allocated twice, and two files sharing a cluster is
 * the classic cross-link that no amount of later checking can undo. */
static unsigned Fat32AllocateCluster(Fat32Volume_t *V)
{
    unsigned Cluster;

    for (Cluster = 2; Cluster < (V->TotalClusters + 2); Cluster++) {
        unsigned Offset, Value;

        if (Fat32LoadFatSector(V, Cluster, &Offset) != FAT32_OK) {
            return 0;
        }

        Value = ((unsigned)V->FatCache[Offset]
              | ((unsigned)V->FatCache[Offset + 1] << 8)
              | ((unsigned)V->FatCache[Offset + 2] << 16)
              | ((unsigned)V->FatCache[Offset + 3] << 24))
              & FAT32_CLUSTER_MASK;

        if (Value == FAT32_CLUSTER_FREE) {
            if (Fat32SetCluster(V, Cluster, FAT32_CLUSTER_EOC) != FAT32_OK) {
                return 0;
            }
            V->ClustersAllocated++;
            return Cluster;
        }
    }

    return 0;
}

/* Fat32ZeroCluster
 * A freshly allocated cluster holds whatever the last file left there.
 * For a directory that is fatal - the stale bytes parse as entries. */
static int Fat32ZeroCluster(Fat32Volume_t *V, unsigned Cluster)
{
    unsigned char Zero[FAT32_SECTOR_SIZE];
    unsigned Base = Fat32ClusterToSector(V, Cluster);
    unsigned s;

    CoreSet(Zero, 0, sizeof(Zero));

    for (s = 0; s < V->SectorsPerCluster; s++) {
        if (V->Write(V->Context, Base + s, 1, Zero) != 0) {
            return FAT32_ERR_WRITE;
        }
    }
    return FAT32_OK;
}

/* ---------------------------------------------------------------- */
/* mount                                                             */
/* ---------------------------------------------------------------- */

int Fat32Mount(Fat32Volume_t *V, Fat32ReadSectors_t Read,
               Fat32WriteSectors_t Write, void *Context)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    Fat32BootSector_t *Boot;
    unsigned DataSectors;

    CoreSet(V, 0, sizeof(Fat32Volume_t));
    V->Read     = Read;
    V->Write    = Write;
    V->Context  = Context;
    V->ReadOnly = (Write == 0) ? 1 : 0;

    if (Read(Context, 0, 1, Sector) != 0) {
        return FAT32_ERR_READ;
    }

    Boot = (Fat32BootSector_t*)Sector;

    if (Sector[510] != 0x55 || Sector[511] != 0xAA) {
        return FAT32_ERR_SIGNATURE;
    }
    if (Boot->BytesPerSector != 512 || Boot->SectorsPerCluster == 0) {
        return FAT32_ERR_GEOMETRY;
    }
    if (Boot->FatCount == 0 || Boot->SectorsPerFat32 == 0) {
        return FAT32_ERR_NOT_FAT32;
    }
    if (Boot->RootEntryCount != 0) {
        return FAT32_ERR_NOT_FAT32;
    }

    V->BytesPerSector    = Boot->BytesPerSector;
    V->SectorsPerCluster = Boot->SectorsPerCluster;
    V->FirstFatSector    = Boot->ReservedSectors;
    V->SectorsPerFat     = Boot->SectorsPerFat32;
    V->FatCount          = Boot->FatCount;
    V->RootCluster       = Boot->RootCluster;
    V->FirstDataSector   = Boot->ReservedSectors
                         + (Boot->FatCount * Boot->SectorsPerFat32);
    V->FsInfoSector      = Boot->FsInfoSector;

    DataSectors = Boot->TotalSectors32 - V->FirstDataSector;
    V->TotalClusters = DataSectors / Boot->SectorsPerCluster;

    if (V->TotalClusters < 65525) {
        return FAT32_ERR_TOO_SMALL;
    }

    V->Mounted = 1;
    return FAT32_OK;
}

/* ---------------------------------------------------------------- */
/* names                                                             */
/* ---------------------------------------------------------------- */

static void Fat32FormatName(const char *Raw, char *Out)
{
    int i, o = 0;

    for (i = 0; i < 8 && Raw[i] != ' '; i++) {
        Out[o++] = Raw[i];
    }
    if (Raw[8] != ' ') {
        Out[o++] = '.';
        for (i = 8; i < 11 && Raw[i] != ' '; i++) {
            Out[o++] = Raw[i];
        }
    }
    Out[o] = '\0';
}

static char Fat32Upper(char c)
{
    return (c >= 'a' && c <= 'z') ? (char)(c - 'a' + 'A') : c;
}

/* Fat32MakeShortName
 * Turns "readme.txt" into the on-disk "README  TXT". Returns 0 if the
 * name will not fit 8.3 - better to refuse than to silently truncate
 * into a different file. */
static int Fat32MakeShortName(const char *Name, char *Out)
{
    int i = 0, o = 0;

    for (o = 0; o < 11; o++) { Out[o] = ' '; }

    for (o = 0; Name[i] != '\0' && Name[i] != '.'; i++) {
        if (o >= 8) { return 0; }
        Out[o++] = Fat32Upper(Name[i]);
    }
    if (o == 0) { return 0; }

    if (Name[i] == '.') {
        i++;
        for (o = 8; Name[i] != '\0'; i++) {
            if (o >= 11) { return 0; }
            Out[o++] = Fat32Upper(Name[i]);
        }
    }
    return 1;
}

static int Fat32NameMatches(const char *Raw, const char *Wanted)
{
    char Formatted[13];
    int i;

    Fat32FormatName(Raw, Formatted);

    for (i = 0; i < 13; i++) {
        char a = Fat32Upper(Formatted[i]);
        char b = Fat32Upper(Wanted[i]);

        if (a != b) { return 0; }
        if (a == '\0') { return 1; }
    }
    return 1;
}

/* ---------------------------------------------------------------- */
/* directories                                                       */
/* ---------------------------------------------------------------- */

typedef int (*Fat32Visit_t)(FatDirEntry_t *, unsigned Sector,
                            unsigned Offset, void *Context);

static int Fat32WalkDirectory(Fat32Volume_t *V, unsigned Cluster,
                              Fat32Visit_t Visit, void *Context)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    unsigned Guard = 0;

    if (!V->Mounted) {
        return FAT32_ERR_READ;
    }
    if (Cluster == 0) {
        Cluster = V->RootCluster;
    }

    while (Cluster >= 2) {
        unsigned Base = Fat32ClusterToSector(V, Cluster);
        unsigned s;

        /* A corrupt FAT can contain a loop; without this the walk never
         * returns and the file server stops answering anything. */
        if (++Guard > 65536) {
            return FAT32_ERR_READ;
        }

        for (s = 0; s < V->SectorsPerCluster; s++) {
            unsigned e;

            if (V->Read(V->Context, Base + s, 1, Sector) != 0) {
                return FAT32_ERR_READ;
            }

            for (e = 0; e < V->BytesPerSector / 32; e++) {
                FatDirEntry_t *Entry = (FatDirEntry_t*)(Sector + (e * 32));

                if ((unsigned char)Entry->Name[0] == FAT_ENTRY_END) {
                    /* Visit it anyway: a creator needs to know where the
                     * free space starts. */
                    Visit(Entry, Base + s, e * 32, Context);
                    return FAT32_OK;
                }
                if ((unsigned char)Entry->Name[0] == FAT_ENTRY_FREE) {
                    Visit(Entry, Base + s, e * 32, Context);
                    continue;
                }
                /* LFN first: an LFN entry has the volume-id bit set too,
                 * so checking for a label first reports garbage names. */
                if ((Entry->Attributes & FAT_ATTR_LFN) == FAT_ATTR_LFN) {
                    continue;
                }
                if (Entry->Attributes & FAT_ATTR_VOLUME_ID) {
                    continue;
                }

                if (Visit(Entry, Base + s, e * 32, Context)) {
                    return FAT32_OK;
                }
            }
        }

        Cluster = Fat32NextCluster(V, Cluster);
    }

    return FAT32_OK;
}

typedef struct { Fat32File_t *Files; int Max; int Count; } ListCtx_t;

static int ListVisit(FatDirEntry_t *E, unsigned Sector, unsigned Offset,
                     void *Ctx)
{
    ListCtx_t *C = (ListCtx_t*)Ctx;
    Fat32File_t *F;

    if ((unsigned char)E->Name[0] == FAT_ENTRY_END
        || (unsigned char)E->Name[0] == FAT_ENTRY_FREE) {
        return 0;
    }
    if (C->Count >= C->Max) {
        return 1;
    }

    F = &C->Files[C->Count++];
    Fat32FormatName(E->Name, F->Name);
    F->FirstCluster = ((unsigned)E->ClusterHigh << 16) | (unsigned)E->ClusterLow;
    F->Size         = E->FileSize;
    F->IsDirectory  = (E->Attributes & FAT_ATTR_DIRECTORY) ? 1 : 0;
    F->EntrySector  = Sector;
    F->EntryOffset  = Offset;
    return 0;
}

int Fat32ListDirectory(Fat32Volume_t *V, unsigned Cluster,
                       Fat32File_t *Files, int Max)
{
    ListCtx_t C;

    C.Files = Files; C.Max = Max; C.Count = 0;
    if (Fat32WalkDirectory(V, Cluster, ListVisit, &C) != FAT32_OK) {
        return -1;
    }
    return C.Count;
}

typedef struct { const char *Name; Fat32File_t *File; int Found; } FindCtx_t;

static int FindVisit(FatDirEntry_t *E, unsigned Sector, unsigned Offset,
                     void *Ctx)
{
    FindCtx_t *C = (FindCtx_t*)Ctx;

    if ((unsigned char)E->Name[0] == FAT_ENTRY_END
        || (unsigned char)E->Name[0] == FAT_ENTRY_FREE) {
        return 0;
    }
    if (!Fat32NameMatches(E->Name, C->Name)) {
        return 0;
    }

    Fat32FormatName(E->Name, C->File->Name);
    C->File->FirstCluster = ((unsigned)E->ClusterHigh << 16)
                          | (unsigned)E->ClusterLow;
    C->File->Size        = E->FileSize;
    C->File->IsDirectory = (E->Attributes & FAT_ATTR_DIRECTORY) ? 1 : 0;
    C->File->EntrySector = Sector;
    C->File->EntryOffset = Offset;
    C->Found = 1;
    return 1;
}

static int Fat32FindIn(Fat32Volume_t *V, unsigned Cluster, const char *Name,
                       Fat32File_t *File)
{
    FindCtx_t C;

    C.Name = Name; C.File = File; C.Found = 0;
    Fat32WalkDirectory(V, Cluster, FindVisit, &C);
    return C.Found;
}

/* Fat32Resolve
 * Splits a path on '/' and walks one component at a time. */
int Fat32Resolve(Fat32Volume_t *V, const char *Path, Fat32File_t *File)
{
    char Component[13];
    unsigned Cluster;
    int i = 0, c;

    if (!V->Mounted) {
        return FAT32_ERR_READ;
    }

    Cluster = V->RootCluster;

    /* The root itself. */
    File->FirstCluster = Cluster;
    File->Size         = 0;
    File->IsDirectory  = 1;
    File->Name[0]      = '/';
    File->Name[1]      = '\0';
    File->EntrySector  = 0;
    File->EntryOffset  = 0;

    while (Path[i] == '/') { i++; }

    while (Path[i] != '\0') {
        for (c = 0; Path[i] != '\0' && Path[i] != '/'; i++) {
            if (c >= 12) {
                return FAT32_ERR_NAME;
            }
            Component[c++] = Path[i];
        }
        Component[c] = '\0';
        while (Path[i] == '/') { i++; }

        if (c == 0) {
            continue;
        }

        if (!Fat32FindIn(V, Cluster, Component, File)) {
            return FAT32_ERR_NOTFOUND;
        }

        /* Another component to go, so this one has to be a directory.
         * Walking into a file would read its contents as entries. */
        if (Path[i] != '\0') {
            if (!File->IsDirectory) {
                return FAT32_ERR_NOTFOUND;
            }
            Cluster = File->FirstCluster;
        }
    }

    return FAT32_OK;
}

/* ---------------------------------------------------------------- */
/* reading                                                           */
/* ---------------------------------------------------------------- */

int Fat32ReadFile(Fat32Volume_t *V, const Fat32File_t *File, unsigned Offset,
                  void *Buffer, unsigned Length)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    unsigned char *Out = (unsigned char*)Buffer;
    unsigned ClusterBytes, Cluster, Skip, Done = 0;

    if (!V->Mounted || File->FirstCluster < 2) {
        return -1;
    }
    if (Offset >= File->Size) {
        return 0;
    }
    if ((Offset + Length) > File->Size) {
        Length = File->Size - Offset;
    }

    ClusterBytes = V->SectorsPerCluster * V->BytesPerSector;
    Cluster = File->FirstCluster;

    Skip = Offset / ClusterBytes;
    while (Skip-- > 0) {
        Cluster = Fat32NextCluster(V, Cluster);
        if (Cluster < 2) { return -1; }
    }
    Offset %= ClusterBytes;

    while (Done < Length && Cluster >= 2) {
        unsigned Base = Fat32ClusterToSector(V, Cluster);
        unsigned s;

        for (s = Offset / V->BytesPerSector;
             s < V->SectorsPerCluster && Done < Length; s++) {
            unsigned In = Offset % V->BytesPerSector;
            unsigned Chunk = V->BytesPerSector - In;

            if (Chunk > (Length - Done)) { Chunk = Length - Done; }
            if (V->Read(V->Context, Base + s, 1, Sector) != 0) {
                return (int)Done;
            }
            CoreCopy(Out + Done, Sector + In, Chunk);
            Done   += Chunk;
            Offset += Chunk;
        }

        Offset = 0;
        Cluster = Fat32NextCluster(V, Cluster);
    }

    return (int)Done;
}

/* ---------------------------------------------------------------- */
/* writing                                                           */
/* ---------------------------------------------------------------- */

/* Fat32UpdateEntry
 * Rewrites the file's directory entry with its current size and first
 * cluster. Read the sector, change 6 bytes, write it back - the other
 * 15 entries in that sector belong to other files. */
static int Fat32UpdateEntry(Fat32Volume_t *V, const Fat32File_t *File)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    FatDirEntry_t *E;

    if (V->Write == 0) { return FAT32_ERR_READONLY; }
    if (File->EntrySector == 0) { return FAT32_ERR_NOTFOUND; }

    if (V->Read(V->Context, File->EntrySector, 1, Sector) != 0) {
        return FAT32_ERR_READ;
    }

    E = (FatDirEntry_t*)(Sector + File->EntryOffset);
    E->FileSize    = File->Size;
    if (E->WriteDate == 0) {
        E->WriteDate = (0 << 9) | (1 << 5) | 1;
    }
    E->ClusterLow  = (unsigned short)(File->FirstCluster & 0xFFFF);
    E->ClusterHigh = (unsigned short)(File->FirstCluster >> 16);

    if (V->Write(V->Context, File->EntrySector, 1, Sector) != 0) {
        return FAT32_ERR_WRITE;
    }
    return FAT32_OK;
}

/* Fat32ChainTo
 * Returns the cluster holding byte <Offset>, extending the chain if it
 * does not reach that far. */
static unsigned Fat32ChainTo(Fat32Volume_t *V, Fat32File_t *File,
                             unsigned Offset, int Extend)
{
    unsigned ClusterBytes = V->SectorsPerCluster * V->BytesPerSector;
    unsigned Want = Offset / ClusterBytes;
    unsigned Cluster = File->FirstCluster;
    unsigned Previous = 0;

    if (Cluster < 2) {
        if (!Extend) { return 0; }
        Cluster = Fat32AllocateCluster(V);
        if (Cluster == 0) { return 0; }
        File->FirstCluster = Cluster;
    }

    while (Want-- > 0) {
        unsigned Next = Fat32NextCluster(V, Cluster);

        if (Next < 2) {
            if (!Extend) { return 0; }

            Previous = Cluster;
            Next = Fat32AllocateCluster(V);
            if (Next == 0) { return 0; }

            /* Link only after the new cluster is marked end-of-chain.
             * The other order leaves the chain pointing at a cluster
             * still marked free, which a concurrent allocation could
             * hand to someone else. */
            if (Fat32SetCluster(V, Previous, Next) != FAT32_OK) {
                return 0;
            }
        }
        Cluster = Next;
    }

    return Cluster;
}

int Fat32WriteFile(Fat32Volume_t *V, Fat32File_t *File, unsigned Offset,
                   const void *Buffer, unsigned Length)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    const unsigned char *In = (const unsigned char*)Buffer;
    unsigned ClusterBytes, Done = 0;

    if (!V->Mounted) { return FAT32_ERR_READ; }
    if (V->Write == 0) { return FAT32_ERR_READONLY; }
    if (File->IsDirectory) { return FAT32_ERR_NOTFOUND; }
    if (Length == 0) { return 0; }

    ClusterBytes = V->SectorsPerCluster * V->BytesPerSector;

    while (Done < Length) {
        unsigned At = Offset + Done;
        unsigned Cluster = Fat32ChainTo(V, File, At, 1);
        unsigned Base, s, InSector, Chunk;

        if (Cluster < 2) {
            break;      /* out of space; report what was written */
        }

        Base     = Fat32ClusterToSector(V, Cluster);
        s        = (At % ClusterBytes) / V->BytesPerSector;
        InSector = At % V->BytesPerSector;
        Chunk    = V->BytesPerSector - InSector;

        if (Chunk > (Length - Done)) { Chunk = Length - Done; }

        /* Read-modify-write unless the whole sector is being replaced.
         * Writing a partial sector without reading first destroys the
         * bytes around it - which for an append is the tail of the
         * previous write. */
        if (Chunk != V->BytesPerSector) {
            if (V->Read(V->Context, Base + s, 1, Sector) != 0) {
                return (Done > 0) ? (int)Done : FAT32_ERR_READ;
            }
        }
        CoreCopy(Sector + InSector, In + Done, Chunk);

        if (V->Write(V->Context, Base + s, 1, Sector) != 0) {
            return (Done > 0) ? (int)Done : FAT32_ERR_WRITE;
        }
        Done += Chunk;
    }

    if ((Offset + Done) > File->Size) {
        File->Size = Offset + Done;
    }

    /* Data first, then the FAT, then the directory entry.
     *
     * This order is the whole crash story. Interrupted after the data,
     * nothing changed. Interrupted after the FAT, some clusters are
     * allocated to nobody - space lost until a checker runs, but no file
     * is wrong. Only the last step makes the new bytes visible. The
     * reverse order would publish a size that points at clusters not yet
     * linked, which reads as garbage. */
    if (Fat32FlushCache(V) != FAT32_OK) {
        return FAT32_ERR_WRITE;
    }
    if (Fat32UpdateEntry(V, File) != FAT32_OK) {
        return FAT32_ERR_WRITE;
    }

    return (int)Done;
}

typedef struct {
    const char *Short;          /* 11 raw bytes */
    int         Exists;
    unsigned    FreeSector;
    unsigned    FreeOffset;
    int         FoundFree;
} CreateCtx_t;

static int CreateVisit(FatDirEntry_t *E, unsigned Sector, unsigned Offset,
                       void *Ctx)
{
    CreateCtx_t *C = (CreateCtx_t*)Ctx;
    int i;

    if ((unsigned char)E->Name[0] == FAT_ENTRY_END
        || (unsigned char)E->Name[0] == FAT_ENTRY_FREE) {
        if (!C->FoundFree) {
            C->FreeSector = Sector;
            C->FreeOffset = Offset;
            C->FoundFree  = 1;
        }
        return 0;
    }

    for (i = 0; i < 11; i++) {
        if (E->Name[i] != C->Short[i]) {
            return 0;
        }
    }
    C->Exists = 1;
    return 1;
}

int Fat32Create(Fat32Volume_t *V, const char *Path, Fat32File_t *File)
{
    unsigned char Sector[FAT32_SECTOR_SIZE];
    char Short[11];
    char Leaf[13];
    char Parent[FAT32_PATH_MAX];
    Fat32File_t Dir;
    CreateCtx_t C;
    FatDirEntry_t *E;
    unsigned DirCluster;
    int i, last = -1, p = 0;

    if (!V->Mounted) { return FAT32_ERR_READ; }
    if (V->Write == 0) { return FAT32_ERR_READONLY; }

    /* Split off the last component. */
    for (i = 0; Path[i] != '\0'; i++) {
        if (i >= FAT32_PATH_MAX - 1) { return FAT32_ERR_NAME; }
        if (Path[i] == '/') { last = i; }
    }
    for (i = 0; i < last; i++) { Parent[p++] = Path[i]; }
    Parent[p] = '\0';

    p = 0;
    for (i = last + 1; Path[i] != '\0'; i++) {
        if (p >= 12) { return FAT32_ERR_NAME; }
        Leaf[p++] = Path[i];
    }
    Leaf[p] = '\0';

    if (!Fat32MakeShortName(Leaf, Short)) {
        return FAT32_ERR_NAME;
    }

    if (last <= 0) {
        DirCluster = V->RootCluster;
    }
    else {
        if (Fat32Resolve(V, Parent, &Dir) != FAT32_OK || !Dir.IsDirectory) {
            return FAT32_ERR_NOTFOUND;
        }
        DirCluster = Dir.FirstCluster;
    }

    C.Short = Short; C.Exists = 0; C.FoundFree = 0;
    C.FreeSector = 0; C.FreeOffset = 0;
    Fat32WalkDirectory(V, DirCluster, CreateVisit, &C);

    if (C.Exists)    { return FAT32_ERR_EXISTS; }
    if (!C.FoundFree) { return FAT32_ERR_FULL; }

    if (V->Read(V->Context, C.FreeSector, 1, Sector) != 0) {
        return FAT32_ERR_READ;
    }

    E = (FatDirEntry_t*)(Sector + C.FreeOffset);
    CoreSet(E, 0, sizeof(FatDirEntry_t));
    for (i = 0; i < 11; i++) { E->Name[i] = Short[i]; }
    E->Attributes  = FAT_ATTR_ARCHIVE;

    /* 1980-01-01. There is no clock to ask, but all-zero is month 0 and
     * day 0 - a date that cannot exist, which tools display as
     * 1980-00-00. The epoch itself is at least a real day. */
    E->CreateDate  = (0 << 9) | (1 << 5) | 1;
    E->WriteDate   = E->CreateDate;
    E->AccessDate  = E->CreateDate;
    E->CreateTime  = 0;
    E->WriteTime   = 0;

    E->FileSize    = 0;
    E->ClusterLow  = 0;
    E->ClusterHigh = 0;

    /* An empty file owns no cluster. Allocating one here would waste a
     * cluster per empty file and, worse, leave a chain the size field
     * says is not there. */

    if (V->Write(V->Context, C.FreeSector, 1, Sector) != 0) {
        return FAT32_ERR_WRITE;
    }

    Fat32FormatName(Short, File->Name);
    File->FirstCluster = 0;
    File->Size         = 0;
    File->IsDirectory  = 0;
    File->EntrySector  = C.FreeSector;
    File->EntryOffset  = C.FreeOffset;

    (void)Fat32ZeroCluster;
    return FAT32_OK;
}
