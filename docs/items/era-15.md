# Era 15 · Devices: a device manager and confined drivers

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: the kernel holds no list of device kinds or vendors, and a new driver is added without changing it; two disks and two network cards work at once, each with its own driver; every driver's device is confined by the IOMMU on the real PC; and a test that kills each driver in the middle of its I/O finds the system recovered. Whose it is, and when, is for the user to say. The drivers the real PC needs (K2 hubs, K5, K6, K8, K9, K14) stay in Era 7's list; this era is about how every driver is found, started, confined and restarted.

Every driver is already a user-space program (`blk`, `nic`, `hid`, `ahci`, `nvme`, `e1000`, `usb`, `kbd`, `serial`, `display`, `power`), its logic in a crate tested on the host against a simulated device (`usb`, `ahci`, `nvme`, `e1000`, `virtio`, `input`), and init restarts one that crashes; init keeps its DMA memory, so a restart never frees memory the device may still write. What is wrong against the Goals:

- **Not pure, not extensible**: the kernel decides which devices are handed out. `offered()` in `devices.rs` matches AHCI, NVMe and xHCI by class and the e1000 by Intel's numbers, and knows details of each (which BAR, MSI-X or not, NVMe's smallest register size); `mmio_devices.rs` identifies virtio devices. Every new driver is a kernel change.
- **Not secure on the bus**: no IOMMU, so a driver can make its device DMA anywhere, the kernel included (`devices.rs` says so). Bus mastering is turned on at hand-out and never off.
- **Not scalable or modular**: init has fixed slots for one disk, one network card and one USB controller, with each driver's startup handles numbered by hand; a second NVMe drive is ignored. One `usb` program is the controller, the keyboards, the mice and the stick at once.
- **Not fast**: one interrupt vector per device, and one fixed contiguous DMA region that data is copied through.

The fix, Zircon's way, keeps the kernel to mechanism: it hands out every device the same way (configuration space, every BAR, its interrupts), and a user-space device manager decides which driver gets it.

| # | Item | Size | Notes |
|---|---|---|---|
| K75 | **Every device handed out the same way**: each PCI function and each device-tree node with its configuration space (a capability to its own), every BAR, and its interrupts; the kernel's lists of classes, vendors and virtio kinds are removed, and with them what it knows of each kind | M | First. A new driver then needs no kernel change |
| K76 | **A device manager** in user space: a table of drivers matched by class and IDs (and by device-tree name on AArch64), one driver started per device, several devices of a kind, restarts; init's fixed slots and hand-numbered handles go | M | After K75. The policy moves from the kernel and init to one table |
| K77 | **Bus mastering tied to a handle**: when the last handle to a device closes (its driver died), the kernel switches its DMA off until a new driver turns it on | S | Before or with K76 |
| K27 | **An IOMMU** (Intel VT-d, AMD-Vi; the SMMU on AArch64) confining each driver's device to its own memory objects | L | Moved here from Security. The kernel programs it from the memory objects a driver holds: still mechanism |
| K78 | **USB as a bus**: the xHCI driver as a bus driver, and keyboards, mice and storage as class drivers of their own; more than one stick | M | With K2 (hubs). A bug in the stick's code no longer takes the keyboard with it |
| K79 | **Several interrupt vectors and queues per device** (MSI-X), one per CPU for NVMe and network cards | M | With K8; after K68 |
| K80 | **DMA into a program's own memory**: a memory object pinned for the device, scatter-gather lists, instead of copying through one fixed region | M | With K27, which keeps it safe |
| K81 | **virtio 1.x** (the modern interface) beside the legacy one | M | Some hypervisors, and virtio-gpu, need it |
| K82 | **A test that kills each driver in the middle of its I/O** - disk, network, USB, input - and checks that the system recovers and no data is lost that was acknowledged | S | Early: it measures the era's milestone |
| K83 | **Device power states** (D0-D3), devices put to sleep when idle | M | Later; laptops, with K18 |
