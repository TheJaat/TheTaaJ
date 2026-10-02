#ifndef __FAT32CORE_H__
#define __FAT32CORE_H__

#include <os/fat32.h>

/* The format logic, with no dependency on how sectors are obtained.
 *
 * The server supplies read and write functions that talk to the disk
 * service; the host test supplies ones that use a file. The same code
 * runs in both, so a bug found on the host is a bug in the OS - and for
 * the write path that matters more than anywhere else, because the
 * oracle is fsck.vfat rather than my own opinion. */

typedef int (*Fat32ReadSectors_t)(void *Context, unsigned Lba,
                                  unsigned Count, void *Buffer);
typedef int (*Fat32WriteSectors_t)(void *Context, unsigned Lba,
                                   unsigned Count, const void *Buffer);

#define FAT32_SECTOR_SIZE   512
#define FAT32_PATH_MAX      128

typedef struct _Fat32Volume {
    Fat32ReadSectors_t  Read;
    Fat32WriteSectors_t Write;
    void          *Context;

    unsigned int   BytesPerSector;
    unsigned int   SectorsPerCluster;
    unsigned int   FirstDataSector;
    unsigned int   FirstFatSector;
    unsigned int   SectorsPerFat;
    unsigned int   FatCount;
    unsigned int   RootCluster;
    unsigned int   TotalClusters;
    int            Mounted;
    int            ReadOnly;        /* no Write function supplied */

    /* One cached FAT sector.
     *
     * Walking a chain reads a FAT sector per link, and consecutive
     * clusters almost always live in the same sector - 128 entries fit
     * in 512 bytes. One sector of cache turns the common case from a
     * disk round trip into a memory read, and the write path needs the
     * sector in memory anyway to modify one entry without destroying
     * the other 127. */
    unsigned char  FatCache[FAT32_SECTOR_SIZE];
    unsigned int   FatCacheSector;  /* 0 = nothing cached */
    int            FatCacheDirty;
    unsigned int   CacheHits;
    unsigned int   CacheMisses;

    /* FSInfo, the free-cluster hint.
     *
     * Nothing in this driver needs it - the allocator scans the FAT
     * itself - but it is part of the on-disk contract, and a volume
     * whose count is stale is a volume fsck calls wrong. Tracking how
     * many clusters we took is cheaper than recounting. */
    unsigned int   FsInfoSector;
    unsigned int   ClustersAllocated;
} Fat32Volume_t;

typedef struct _Fat32File {
    char           Name[13];        /* 8.3 with the dot, NUL terminated */
    unsigned int   FirstCluster;
    unsigned int   Size;
    int            IsDirectory;

    /* Where the directory entry itself lives, so a write can update the
     * size without searching for it again. */
    unsigned int   EntrySector;
    unsigned int   EntryOffset;
} Fat32File_t;

#define FAT32_OK                 0
#define FAT32_ERR_READ          -1
#define FAT32_ERR_SIGNATURE     -2
#define FAT32_ERR_GEOMETRY      -3
#define FAT32_ERR_NOT_FAT32     -4
#define FAT32_ERR_TOO_SMALL     -5
#define FAT32_ERR_WRITE         -6
#define FAT32_ERR_FULL          -7
#define FAT32_ERR_READONLY      -8
#define FAT32_ERR_NOTFOUND      -9
#define FAT32_ERR_EXISTS       -10
#define FAT32_ERR_NAME         -11

int  Fat32Mount(Fat32Volume_t *, Fat32ReadSectors_t, Fat32WriteSectors_t,
                void *Context);
const char *Fat32Error(int Code);

/* Fat32Flush
 * Writes the cached FAT sector back if it is dirty. Must be called
 * before the volume is abandoned, or the last allocation is lost. */
int  Fat32Flush(Fat32Volume_t *);

unsigned Fat32NextCluster(Fat32Volume_t *, unsigned Cluster);

int  Fat32ListDirectory(Fat32Volume_t *, unsigned Cluster,
                        Fat32File_t *Files, int Max);

/* Fat32Resolve
 * Walks a path such as "/sub/dir/file.txt" from the root. A leading
 * slash is optional. Returns FAT32_OK or an error. */
int  Fat32Resolve(Fat32Volume_t *, const char *Path, Fat32File_t *File);

int  Fat32ReadFile(Fat32Volume_t *, const Fat32File_t *, unsigned Offset,
                   void *Buffer, unsigned Length);

/* Fat32WriteFile
 * Writes at Offset, extending the file and its chain as needed. Returns
 * the number of bytes written, or a negative error. */
int  Fat32WriteFile(Fat32Volume_t *, Fat32File_t *, unsigned Offset,
                    const void *Buffer, unsigned Length);

/* Fat32Create
 * Creates an empty file in the directory containing Path. Fails if the
 * name already exists. */
int  Fat32Create(Fat32Volume_t *, const char *Path, Fat32File_t *File);

#endif
