#ifndef __FAT32CORE_H__
#define __FAT32CORE_H__

#include <os/fat32.h>

/* The format logic, with no dependency on how sectors are obtained.
 *
 * The server supplies a read function that talks to the disk service;
 * the host test supplies one that reads a file. The same code runs in
 * both, so a bug found on the host is a bug in the OS. */

typedef int (*Fat32ReadSectors_t)(void *Context, unsigned Lba,
                                  unsigned Count, void *Buffer);

typedef struct _Fat32Volume {
    Fat32ReadSectors_t Read;
    void          *Context;

    unsigned int   BytesPerSector;
    unsigned int   SectorsPerCluster;
    unsigned int   FirstDataSector;
    unsigned int   FirstFatSector;
    unsigned int   RootCluster;
    unsigned int   TotalClusters;
    int            Mounted;
} Fat32Volume_t;

typedef struct _Fat32File {
    char           Name[13];        /* 8.3 with the dot, NUL terminated */
    unsigned int   FirstCluster;
    unsigned int   Size;
    int            IsDirectory;
} Fat32File_t;

/* Mount failures are distinguished rather than collapsed into -1. "No
 * FAT32 volume" is true of a blank disk, a FAT16 disk, a disk whose
 * driver is returning garbage and a disk that is not being read at all,
 * and those need different fixes. */
#define FAT32_OK                 0
#define FAT32_ERR_READ          -1   /* the device read failed          */
#define FAT32_ERR_SIGNATURE     -2   /* no 0x55AA - blank or not a disk */
#define FAT32_ERR_GEOMETRY      -3   /* impossible sector/cluster size  */
#define FAT32_ERR_NOT_FAT32     -4   /* FAT12/16 layout                 */
#define FAT32_ERR_TOO_SMALL     -5   /* under 65525 clusters            */

int Fat32Mount(Fat32Volume_t *Volume, Fat32ReadSectors_t Read, void *Context);
const char *Fat32MountError(int Code);

/* Fat32NextCluster
 * Follows the chain. Returns 0 at the end or on a bad entry. */
unsigned Fat32NextCluster(Fat32Volume_t *Volume, unsigned Cluster);

/* Fat32ListDirectory
 * Fills up to Max entries from the directory starting at Cluster.
 * Pass the root cluster for the root. Returns how many were found. */
int Fat32ListDirectory(Fat32Volume_t *Volume, unsigned Cluster,
                       Fat32File_t *Files, int Max);

/* Fat32Find
 * Looks up one 8.3 name in a directory. Returns 1 on success. */
int Fat32Find(Fat32Volume_t *Volume, unsigned Cluster, const char *Name,
              Fat32File_t *File);

/* Fat32ReadFile
 * Reads up to Length bytes from Offset. Returns bytes read. */
int Fat32ReadFile(Fat32Volume_t *Volume, const Fat32File_t *File,
                  unsigned Offset, void *Buffer, unsigned Length);

#endif
