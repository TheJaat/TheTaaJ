/* pci - bus enumeration, in ring 3.
 *
 * Needs nothing but two io ports, so it is a driver that could have been
 * written the day port delegation landed. It publishes an RPC service
 * that other drivers use to find their hardware. */

#include <os/rpc.h>
#include <os/pcisrv.h>

#define PCI_ADDRESS     0xCF8
#define PCI_DATA        0xCFC

#define PCI_MAX_DEVICES 32

static PciDevice_t Devices[PCI_MAX_DEVICES];
static unsigned int Count = 0;

/* PciConfigRead
 * The configuration mechanism is two 32-bit ports: write a selector to
 * 0xCF8, read the dword at 0xCFC.
 *
 * Bit 31 is the enable bit and the offset's low two bits must be zero -
 * the mechanism is dword-addressed, and leaving them set selects a
 * different register than intended. */
static unsigned int PciConfigRead(unsigned bus, unsigned dev, unsigned fn,
                                  unsigned offset)
{
    unsigned int address = (1u << 31)
                         | (bus << 16) | (dev << 11) | (fn << 8)
                         | (offset & 0xFC);

    /* One 32-bit write, then one 32-bit read. This is not an
     * optimisation over four byte accesses - byte writes to 0xCF8 do
     * not assemble into a dword address, the controller never latches a
     * complete selector, and every read then returns the same value for
     * every slot. */
    SysOutL(PCI_ADDRESS, address);
    return SysInL(PCI_DATA);
}

static void PciScan(void)
{
    unsigned bus, dev, fn;

    for (bus = 0; bus < 4; bus++) {
        for (dev = 0; dev < 32; dev++) {
            for (fn = 0; fn < 8; fn++) {
                unsigned int id = PciConfigRead(bus, dev, fn, 0x00);
                unsigned short vendor = (unsigned short)(id & 0xFFFF);
                PciDevice_t *d;
                unsigned int classes, header;
                int b;

                /* 0xFFFF means no device responded. */
                if (vendor == 0xFFFF) {
                    if (fn == 0) {
                        break;      /* no function 0, no device at all */
                    }
                    continue;
                }
                if (Count >= PCI_MAX_DEVICES) {
                    return;
                }

                d = &Devices[Count++];
                d->Bus = (unsigned char)bus;
                d->Device = (unsigned char)dev;
                d->Function = (unsigned char)fn;
                d->VendorId = vendor;
                d->DeviceId = (unsigned short)(id >> 16);

                classes = PciConfigRead(bus, dev, fn, 0x08);
                d->Revision = (unsigned char)(classes & 0xFF);
                d->ProgIf   = (unsigned char)((classes >> 8) & 0xFF);
                d->Subclass = (unsigned char)((classes >> 16) & 0xFF);
                d->Class    = (unsigned char)((classes >> 24) & 0xFF);

                for (b = 0; b < 6; b++) {
                    d->Bar[b] = PciConfigRead(bus, dev, fn, 0x10 + (b * 4));
                }

                SysPrint("[pci] ");
                SysPrintNumber(bus); SysPrint(":");
                SysPrintNumber(dev); SysPrint(".");
                SysPrintNumber(fn);
                SysPrint("  vendor ");
                SysPrintNumber(d->VendorId);
                SysPrint(" device ");
                SysPrintNumber(d->DeviceId);
                SysPrint(" class ");
                SysPrintNumber(d->Class);
                SysPrint("\n");

                /* Bit 7 of the header type says whether this is a
                 * multi-function device. Scanning all eight functions of
                 * a single-function device reads back the same one
                 * eight times. */
                if (fn == 0) {
                    header = PciConfigRead(bus, dev, fn, 0x0C);
                    if (!(((header >> 16) & 0xFF) & 0x80)) {
                        break;
                    }
                }
            }
        }
    }
}

int ModuleMain(void)
{
    RpcMessage_t Request;
    int Service;

    SysPrint("[pci] starting in ring 3, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    if (SysIoRequest(PCI_ADDRESS, 8) != SYSCALL_OK) {
        SysPrintLine("[pci] denied io ports");
        SysExit(1);
    }
    if (RpcInitialize() != SYSCALL_OK) {
        SysExit(1);
    }

    PciScan();
    SysPrint("[pci] found ");
    SysPrintNumber(Count);
    SysPrintLine(" devices");

    Service = RpcCreateService(PCISRV_NAME);
    if (Service < 0) {
        SysPrintLine("[pci] could not publish");
        SysExit(1);
    }
    SysPrintLine("[pci] published as 'pci'");

    for (;;) {
        if (RpcListen(Service, &Request) < 0) {
            break;
        }

        switch (Request.Header.Opcode) {
            case PCI_OP_COUNT: {
                PciCount_t r; r.Count = Count;
                RpcRespond(&Request, &r, sizeof(r));
                break;
            }
            case PCI_OP_GET: {
                PciIndex_t *q = (PciIndex_t*)Request.Payload;
                if (Request.Header.Length < sizeof(PciIndex_t)
                    || q->Index >= Count) {
                    RpcRespond(&Request, 0, 0);
                }
                else {
                    RpcRespond(&Request, &Devices[q->Index],
                               sizeof(PciDevice_t));
                }
                break;
            }
            case PCI_OP_READ: {
                PciConfig_t *q = (PciConfig_t*)Request.Payload;
                PciConfigResult_t r;
                if (Request.Header.Length < sizeof(PciConfig_t)) {
                    r.Value = 0xFFFFFFFF;
                }
                else {
                    r.Value = PciConfigRead(q->Bus, q->Device,
                                            q->Function, q->Offset);
                }
                RpcRespond(&Request, &r, sizeof(r));
                break;
            }
            default:
                RpcRespond(&Request, 0, 0);
                break;
        }
    }

    SysExit(0);
    return 0;
}
