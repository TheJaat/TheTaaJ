/* FAT32, read path. No dependency on how sectors arrive. */

#include "fat32core.h"

#define SECTOR_MAX  512

static void CoreMemCopy(void *Dst, const void *Src, unsigned Length)
{
    unsigned char *d = (unsigned char*)Dst;
    const unsigned char *s = (const unsigned char*)Src;
    unsigned i;
    for (i = 0; i < Length; i++) { d[i] = s[i]; }
}

/* Fat32ClusterToSector
 * Clusters are numbered from 2. The two missing entries are not a
 * mistake in the format: entries 0 and 1 of the FAT hold the media
 * descriptor and flags, so data clusters start where they do. */
static unsigned Fat32ClusterToSector(Fat32Volume_t *Volume, unsigned Cluster)
{
    return Volume->FirstDataSector
         + ((Cluster - 2) * Volume->SectorsPerCluster);
}

/* Fat32Mount */
int Fat32Mount(Fat32Volume_t *Volume, Fat32ReadSectors_t Read, void *Context)
{
    unsigned char Sector[SECTOR_MAX];
    Fat32BootSector_t *Boot;
    unsigned DataSectors;

    Volume->Mounted = 0;
    Volume->Read    = Read;
    Volume->Context = Context;

    if (Read(Context, 0, 1, Sector) != 0) {
        return FAT32_ERR_READ;
    }

    Boot = (Fat32BootSector_t*)Sector;

    /* The boot signature is the first thing to check: a disk with no
     * filesystem at all usually reads back as zeros, and every field
     * below would then be zero and produce a plausible-looking but
     * entirely wrong geometry. */
    if (Sector[510] != 0x55 || Sector[511] != 0xAA) {
        return FAT32_ERR_SIGNATURE;
    }
    if (Boot->BytesPerSector != 512 || Boot->SectorsPerCluster == 0) {
        return FAT32_ERR_GEOMETRY;
    }
    if (Boot->FatCount == 0 || Boot->SectorsPerFat32 == 0) {
        return FAT32_ERR_NOT_FAT32;   /* zero on FAT12/16 */
    }
    /* RootEntryCount must be zero on FAT32 - a non-zero value means a
     * fixed root directory, which is FAT12/16 and a different layout. */
    if (Boot->RootEntryCount != 0) {
        return FAT32_ERR_NOT_FAT32;
    }

    Volume->BytesPerSector    = Boot->BytesPerSector;
    Volume->SectorsPerCluster = Boot->SectorsPerCluster;
    Volume->FirstFatSector    = Boot->ReservedSectors;
    Volume->RootCluster       = Boot->RootCluster;
    Volume->FirstDataSector   = Boot->ReservedSectors
                              + (Boot->FatCount * Boot->SectorsPerFat32);

    DataSectors = Boot->TotalSectors32 - Volume->FirstDataSector;
    Volume->TotalClusters = DataSectors / Boot->SectorsPerCluster;

    /* The cluster count is what actually decides FAT12 vs 16 vs 32 -
     * the name in the boot sector is advisory and often wrong. Under
     * 65525 clusters this is not FAT32 whatever it claims. */
    if (Volume->TotalClusters < 65525) {
        return FAT32_ERR_TOO_SMALL;
    }

    Volume->Mounted = 1;
    return FAT32_OK;
}

/* Fat32MountError */
const char *Fat32MountError(int Code)
{
    switch (Code) {
        case FAT32_OK:            return "ok";
        case FAT32_ERR_READ:      return "the device read failed";
        case FAT32_ERR_SIGNATURE: return "no 0x55AA signature in sector 0";
        case FAT32_ERR_GEOMETRY:  return "impossible sector or cluster size";
        case FAT32_ERR_NOT_FAT32: return "FAT12/16 layout, not FAT32";
        case FAT32_ERR_TOO_SMALL: return "fewer than 65525 clusters";
        default:                  return "unknown";
    }
}

/* Fat32NextCluster */
unsigned Fat32NextCluster(Fat32Volume_t *Volume, unsigned Cluster)
{
    unsigned char Sector[SECTOR_MAX];
    unsigned FatOffset, FatSector, EntryOffset, Next;

    if (!Volume->Mounted || Cluster < 2) {
        return 0;
    }

    /* Four bytes per entry on FAT32. */
    FatOffset   = Cluster * 4;
    FatSector   = Volume->FirstFatSector + (FatOffset / Volume->BytesPerSector);
    EntryOffset = FatOffset % Volume->BytesPerSector;

    if (Volume->Read(Volume->Context, FatSector, 1, Sector) != 0) {
        return 0;
    }

    Next = (unsigned)Sector[EntryOffset]
         | ((unsigned)Sector[EntryOffset + 1] << 8)
         | ((unsigned)Sector[EntryOffset + 2] << 16)
         | ((unsigned)Sector[EntryOffset + 3] << 24);

    /* The top four bits are reserved and must be ignored. Treating the
     * entry as a full 32-bit value makes every end-of-chain marker look
     * like a wild cluster number. */
    Next &= FAT32_CLUSTER_MASK;

    if (Next >= FAT32_CLUSTER_BAD) {
        return 0;               /* end of chain, or a bad cluster */
    }
    return Next;
}

/* Fat32FormatName
 * 8.3 on disk is space-padded with no dot: "README  TXT". Turn it into
 * something printable. */
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

static int Fat32NameMatches(const char *Raw, const char *Wanted)
{
    char Formatted[13];
    int i;

    Fat32FormatName(Raw, Formatted);

    for (i = 0; i < 13; i++) {
        char a = Formatted[i];
        char b = Wanted[i];

        /* Case-insensitive: 8.3 names are stored upper-case, and
         * nobody types them that way. */
        if (a >= 'a' && a <= 'z') { a = (char)(a - 'a' + 'A'); }
        if (b >= 'a' && b <= 'z') { b = (char)(b - 'a' + 'A'); }

        if (a != b) { return 0; }
        if (a == '\0') { return 1; }
    }
    return 1;
}

/* Fat32WalkDirectory
 * Shared by listing and lookup. Calls Visit for each real entry; Visit
 * returns non-zero to stop. */
static int Fat32WalkDirectory(Fat32Volume_t *Volume, unsigned Cluster,
    int (*Visit)(FatDirEntry_t *, void *), void *Context)
{
    unsigned char Sector[SECTOR_MAX];
    unsigned Guard = 0;

    if (!Volume->Mounted) {
        return -1;
    }
    if (Cluster == 0) {
        Cluster = Volume->RootCluster;
    }

    while (Cluster >= 2) {
        unsigned Base = Fat32ClusterToSector(Volume, Cluster);
        unsigned s;

        /* A corrupt FAT can contain a loop. Without this the walk never
         * returns and the server stops answering anything. */
        if (++Guard > 65536) {
            return -1;
        }

        for (s = 0; s < Volume->SectorsPerCluster; s++) {
            unsigned e;

            if (Volume->Read(Volume->Context, Base + s, 1, Sector) != 0) {
                return -1;
            }

            for (e = 0; e < Volume->BytesPerSector / 32; e++) {
                FatDirEntry_t *Entry = (FatDirEntry_t*)(Sector + (e * 32));

                if ((unsigned char)Entry->Name[0] == FAT_ENTRY_END) {
                    return 0;       /* nothing after this, ever */
                }
                if ((unsigned char)Entry->Name[0] == FAT_ENTRY_FREE) {
                    continue;
                }
                /* Long-filename fragments and the volume label are not
                 * files. The LFN test must come first: an LFN entry has
                 * the volume-id bit set too, and mistaking one for a
                 * label produces entries with garbage names. */
                if ((Entry->Attributes & FAT_ATTR_LFN) == FAT_ATTR_LFN) {
                    continue;
                }
                if (Entry->Attributes & FAT_ATTR_VOLUME_ID) {
                    continue;
                }

                if (Visit(Entry, Context)) {
                    return 0;
                }
            }
        }

        Cluster = Fat32NextCluster(Volume, Cluster);
    }

    return 0;
}

typedef struct _ListContext {
    Fat32File_t *Files;
    int          Max;
    int          Count;
} ListContext_t;

static int ListVisit(FatDirEntry_t *Entry, void *ContextPtr)
{
    ListContext_t *Context = (ListContext_t*)ContextPtr;
    Fat32File_t *File;

    if (Context->Count >= Context->Max) {
        return 1;
    }

    File = &Context->Files[Context->Count++];
    Fat32FormatName(Entry->Name, File->Name);
    File->FirstCluster = ((unsigned)Entry->ClusterHigh << 16)
                       | (unsigned)Entry->ClusterLow;
    File->Size         = Entry->FileSize;
    File->IsDirectory  = (Entry->Attributes & FAT_ATTR_DIRECTORY) ? 1 : 0;
    return 0;
}

int Fat32ListDirectory(Fat32Volume_t *Volume, unsigned Cluster,
                       Fat32File_t *Files, int Max)
{
    ListContext_t Context;

    Context.Files = Files;
    Context.Max   = Max;
    Context.Count = 0;

    if (Fat32WalkDirectory(Volume, Cluster, ListVisit, &Context) != 0) {
        return -1;
    }
    return Context.Count;
}

typedef struct _FindContext {
    const char  *Name;
    Fat32File_t *File;
    int          Found;
} FindContext_t;

static int FindVisit(FatDirEntry_t *Entry, void *ContextPtr)
{
    FindContext_t *Context = (FindContext_t*)ContextPtr;

    if (!Fat32NameMatches(Entry->Name, Context->Name)) {
        return 0;
    }

    Fat32FormatName(Entry->Name, Context->File->Name);
    Context->File->FirstCluster = ((unsigned)Entry->ClusterHigh << 16)
                                | (unsigned)Entry->ClusterLow;
    Context->File->Size        = Entry->FileSize;
    Context->File->IsDirectory = (Entry->Attributes & FAT_ATTR_DIRECTORY) ? 1 : 0;
    Context->Found = 1;
    return 1;
}

int Fat32Find(Fat32Volume_t *Volume, unsigned Cluster, const char *Name,
              Fat32File_t *File)
{
    FindContext_t Context;

    Context.Name  = Name;
    Context.File  = File;
    Context.Found = 0;

    if (Fat32WalkDirectory(Volume, Cluster, FindVisit, &Context) != 0) {
        return 0;
    }
    return Context.Found;
}

/* Fat32ReadFile */
int Fat32ReadFile(Fat32Volume_t *Volume, const Fat32File_t *File,
                  unsigned Offset, void *Buffer, unsigned Length)
{
    unsigned char Sector[SECTOR_MAX];
    unsigned char *Out = (unsigned char*)Buffer;
    unsigned ClusterBytes, Cluster, Skip, Done = 0;

    if (!Volume->Mounted || File->FirstCluster < 2) {
        return -1;
    }
    if (Offset >= File->Size) {
        return 0;
    }
    if ((Offset + Length) > File->Size) {
        Length = File->Size - Offset;   /* never read past the end */
    }

    ClusterBytes = Volume->SectorsPerCluster * Volume->BytesPerSector;
    Cluster = File->FirstCluster;

    /* Walk, rather than index: the chain is a linked list and there is
     * no way to jump into the middle of it. */
    Skip = Offset / ClusterBytes;
    while (Skip-- > 0) {
        Cluster = Fat32NextCluster(Volume, Cluster);
        if (Cluster < 2) {
            return -1;          /* chain shorter than the size claims */
        }
    }

    Offset %= ClusterBytes;

    while (Done < Length && Cluster >= 2) {
        unsigned Base = Fat32ClusterToSector(Volume, Cluster);
        unsigned s;

        for (s = Offset / Volume->BytesPerSector;
             s < Volume->SectorsPerCluster && Done < Length; s++) {
            unsigned InSector = Offset % Volume->BytesPerSector;
            unsigned Chunk = Volume->BytesPerSector - InSector;
            unsigned i;

            if (Chunk > (Length - Done)) {
                Chunk = Length - Done;
            }
            if (Volume->Read(Volume->Context, Base + s, 1, Sector) != 0) {
                return (int)Done;
            }

            for (i = 0; i < Chunk; i++) {
                Out[Done + i] = Sector[InSector + i];
            }
            Done   += Chunk;
            Offset += Chunk;
        }

        Offset = 0;
        Cluster = Fat32NextCluster(Volume, Cluster);
    }

    (void)CoreMemCopy;
    return (int)Done;
}
