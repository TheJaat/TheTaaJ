#ifndef __SHARED_FAT32_H__
#define __SHARED_FAT32_H__

/* On-disk FAT32 structures.
 *
 * Shared with the host test, which runs this same parsing code against
 * an image built by mkfs.vfat. That is the whole reason for choosing an
 * existing format first: when a read comes back wrong, an independent
 * implementation settles whether the disk or the code is at fault. */

typedef struct _Fat32BootSector {
    unsigned char  JumpCode[3];
    char           OemName[8];
    unsigned short BytesPerSector;
    unsigned char  SectorsPerCluster;
    unsigned short ReservedSectors;
    unsigned char  FatCount;
    unsigned short RootEntryCount;      /* 0 on FAT32 */
    unsigned short TotalSectors16;      /* 0 on FAT32 */
    unsigned char  MediaType;
    unsigned short SectorsPerFat16;     /* 0 on FAT32 */
    unsigned short SectorsPerTrack;
    unsigned short HeadCount;
    unsigned int   HiddenSectors;
    unsigned int   TotalSectors32;

    /* FAT32 extension */
    unsigned int   SectorsPerFat32;
    unsigned short Flags;
    unsigned short Version;
    unsigned int   RootCluster;
    unsigned short FsInfoSector;
    unsigned short BackupBootSector;
    unsigned char  Reserved[12];
    unsigned char  DriveNumber;
    unsigned char  NtFlags;
    unsigned char  Signature;
    unsigned int   VolumeId;
    char           VolumeLabel[11];
    char           SystemId[8];
} __attribute__((packed)) Fat32BootSector_t;

typedef struct _FatDirEntry {
    char           Name[11];        /* 8.3, space padded, no dot */
    unsigned char  Attributes;
    unsigned char  NtReserved;
    unsigned char  CreateTenths;
    unsigned short CreateTime;
    unsigned short CreateDate;
    unsigned short AccessDate;
    unsigned short ClusterHigh;
    unsigned short WriteTime;
    unsigned short WriteDate;
    unsigned short ClusterLow;
    unsigned int   FileSize;
} __attribute__((packed)) FatDirEntry_t;

#define FAT_ATTR_READ_ONLY      0x01
#define FAT_ATTR_HIDDEN         0x02
#define FAT_ATTR_SYSTEM         0x04
#define FAT_ATTR_VOLUME_ID      0x08
#define FAT_ATTR_DIRECTORY      0x10
#define FAT_ATTR_ARCHIVE        0x20

/* An entry with all four of these set is a long-filename fragment, not
 * a file. The combination is deliberately impossible for a real entry,
 * which is how LFN stays invisible to readers that predate it. */
#define FAT_ATTR_LFN            0x0F

#define FAT_ENTRY_FREE          0xE5
#define FAT_ENTRY_END           0x00

/* Cluster values. Anything at or above BAD is an end-of-chain marker in
 * practice; only the low 28 bits are meaningful. */
#define FAT32_CLUSTER_MASK      0x0FFFFFFF
#define FAT32_CLUSTER_FREE      0x00000000
#define FAT32_CLUSTER_BAD       0x0FFFFFF7
#define FAT32_CLUSTER_EOC       0x0FFFFFF8

#endif
