#ifndef __SHARED_PCISRV_H__
#define __SHARED_PCISRV_H__

#define PCISRV_NAME         "pci"

#define PCI_OP_COUNT        1   /* -> PciCount_t                     */
#define PCI_OP_GET          2   /* PciIndex_t -> PciDevice_t         */
#define PCI_OP_READ         3   /* PciConfig_t -> PciConfigResult_t  */

typedef struct _PciCount   { unsigned int Count; } PciCount_t;
typedef struct _PciIndex   { unsigned int Index; } PciIndex_t;

typedef struct _PciDevice {
    unsigned char  Bus, Device, Function;
    unsigned short VendorId, DeviceId;
    unsigned char  Class, Subclass, ProgIf, Revision;
    unsigned int   Bar[6];
} PciDevice_t;

typedef struct _PciConfig {
    unsigned char Bus, Device, Function, Offset;
} PciConfig_t;

typedef struct _PciConfigResult { unsigned int Value; } PciConfigResult_t;

#endif
