#ifndef __SHARED_DISKSRV_H__
#define __SHARED_DISKSRV_H__

/* The block device interface.
 *
 * Data never travels in a message. The caller maps the service's shared
 * region, and a request carries only an LBA, a count and an offset into
 * that region. A 4 KB read is one 16-byte message and zero copies of the
 * data itself - which is the entire reason shared memory exists. */

#define DISKSRV_NAME        "disk"

#define DISK_OP_INFO        1   /* no payload   -> DiskInfo_t     */
#define DISK_OP_READ        2   /* DiskRequest_t -> DiskResult_t  */
#define DISK_OP_WRITE       3   /* DiskRequest_t -> DiskResult_t  */

#define DISK_SECTOR_SIZE    512

/* The shared window. Sixteen sectors is enough for a FAT32 cluster at
 * the usual 4 KB, with room to spare. */
#define DISK_WINDOW_SECTORS 16
#define DISK_WINDOW_BYTES   (DISK_WINDOW_SECTORS * DISK_SECTOR_SIZE)

typedef struct _DiskInfo {
    int          Present;
    unsigned int Sectors;        /* total, from IDENTIFY */
    unsigned int SectorSize;
    char         Model[42];
} DiskInfo_t;

typedef struct _DiskRequest {
    unsigned int Lba;
    unsigned int Count;          /* sectors, 1..DISK_WINDOW_SECTORS */
    unsigned int WindowOffset;   /* byte offset into the shared region */
} DiskRequest_t;

typedef struct _DiskResult {
    int          Status;         /* 0 ok, negative on failure */
    unsigned int Count;          /* sectors actually transferred */
} DiskResult_t;

#endif
