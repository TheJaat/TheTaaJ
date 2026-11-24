# NVMe disks: a driver for the other disk a modern PC has

*Journal entry #85 · Era 7, step R2. The kernel now also hands on any NVMe controller: its registers (BAR 0, cut short of the MSI-X table that often shares that BAR) and an interrupt. A new driver, `nvme`, sets up its queues, finds its first namespace and serves it with the same disk protocol as `blk` and `ahci`, so the file service still cannot tell which disk it is on. On QEMU it works on an old PC (BIOS) and on a q35 (UEFI) whose controller moves at most 16 KiB a command. It worked on the first boot. The bugs this time were all in my tests.*

## Why

Era 7's goal is booting TheTaaJ from a USB stick on my own PC and using it for a day. R1 gave it SATA disks. Most PCs sold in the last several years have no SATA disk at all, though: their SSD is an M.2 stick that speaks **NVMe** directly over PCI Express. If TheTaaJ is to keep its files on the disk my PC actually has, it needs this driver. R1 also left the plumbing ready: a memory BAR as a memory object, MSI and MSI-X, `taaj::block_service`. So R2 needed almost nothing new in the kernel.

## What it is

```
 kernel (TheKernel)              init                    nvme (user/src/bin)          fs
 ------------------              ----                    -------------------          --
 PCI scan: class 01.08.02 ─►    "pci" + description ─►  start: nvme::Disk::start ─►  abi::block requests
 BAR 0, up to the MSI-X table    registers: READ|WRITE    (nvme crate: queues, the     (the same as with blk
 37 pages of DMA memory          starts "nvme" as the     whole sequence, host-tested)  and ahci)
 MSI-X (or MSI) -> a line        disk driver              block_service::serve
```

- **The kernel knows only PCI.** A function whose class is 01.08.02 (storage, non-volatile memory, NVMe) is offered, whoever made it. Its registers are BAR 0's memory (a 64-bit BAR; under UEFI on the q35 it lies at 0xc0_0000_0000, above 4 GiB). They are handed on as a memory object, mapped uncached, **but only up to the first MSI-X structure in that BAR** (see "Design decisions").
- **The `nvme` crate** (TheTaaJ, beside `ahci/` and `virtio/`) holds the register map, the command and answer layouts, PRPs, reading IDENTIFY answers, and `Disk`, the whole sequence. It is pure logic against a trait (`Bus`), tested on the host against a simulated controller.
- **The `nvme` program** gives the crate the real registers, the DMA memory and a way to wait, then serves the namespace through `block_service::serve`, exactly as `ahci` does.
- **init** starts `nvme` when the disk it was handed has NVMe's class.

## How NVMe works

AHCI was designed for spinning disks: one command list per port, and registers polled one by one. NVMe was designed for flash. It talks to the controller through **queues in memory**, many of them, each up to 64K entries deep. The registers are few:

| Register | Offset | What it is |
|---|---|---|
| CAP | 0x00 | capabilities: the deepest queue (MQES), how long enabling may take (TO, in 500 ms units), how far apart the doorbells are (DSTRD), which command sets, the smallest page (MPSMIN) |
| VS | 0x08 | the version: 1.4 on QEMU |
| CC | 0x14 | configuration: EN (on/off), the page size, the entry sizes (64-byte commands, 16-byte answers) |
| CSTS | 0x1C | status: RDY (on), CFS (the controller has failed) |
| AQA, ASQ, ACQ | 0x24-0x37 | the admin queues: their sizes and physical addresses |
| doorbells | 0x1000 on | for each queue pair: the SQ's tail, then the CQ's head, `4 << DSTRD` bytes apart |

A **queue pair** is a submission queue (SQ, commands of 64 bytes) and a completion queue (CQ, answers of 16 bytes), each a ring in memory. To run a command, the driver:
1. writes it into the SQ at its tail;
2. moves the tail on and writes the new tail to the SQ's doorbell.

The controller reads the command by DMA, does it, and writes an answer into the CQ. Then it interrupts.

How does the driver know an answer is *new*? It doesn't clear the CQ. Each answer carries a **phase bit**, and the controller writes 1s on its first pass round the ring, 0s on the next, and so on. The driver expects phase 1 until its head wraps past the end, then 0. An entry whose phase is the expected one is new. After reading it, the driver writes its new head to the CQ's doorbell, so the controller knows that slot is free again.

Taking a controller over (`Disk::start`):
1. **Off first:** CC.EN = 0, then wait until CSTS.RDY is 0. That forgets every queue, including the firmware's or an old driver's.
2. **The admin queues:** zero them, then write AQA (16 entries each), ASQ and ACQ.
3. **On:** CC = EN, NVM command set, 4 KiB pages, entry sizes 2^6 and 2^4. Wait for RDY, or for CFS (failed), for at most CAP.TO × 500 ms.
4. **IDENTIFY controller** (CNS 1): the model name and **MDTS**, the most data one command may move, as a power of two of pages.
5. **IDENTIFY the active namespace list** (CNS 2), and take the first. A controller older than NVMe 1.1 refuses CNS 2; then it is namespace 1.
6. **IDENTIFY that namespace** (CNS 0): its size in blocks, and which of its LBA formats is in use (FLBAS). That format's entry gives the block size, as a power of two. TheTaaJ wants 512.
7. **The I/O queues:** *Create I/O Completion Queue* (with interrupts on, vector 0, if there is an interrupt), then *Create I/O Submission Queue*, which names that CQ. Both are 16 deep, physically contiguous, a page each.

```
 DMA memory (37 pages)
 0x0000  admin SQ        16 commands of 64 bytes
 0x1000  admin CQ        16 answers of 16 bytes
 0x2000  I/O SQ          queue 1
 0x3000  I/O CQ
 0x4000  PRP list        the physical address of every data page after the first, written once
 0x5000  data            128 KiB: what a read brings or a write sends
```

### PRPs: where the data is

An NVMe command says where its data is with two **PRPs** (physical region pages):
- **PRP1** is where the data starts.
- If the data runs into a second page, **PRP2** is that page.
- If it runs into more pages than two, PRP2 instead points to a **PRP list**: 8-byte entries, each the physical address of the next page.

My data buffer is contiguous, so the list never changes: entry *k* is the address of data page *k + 1*. It is written once, at start. A command that starts *n* pages into the buffer points PRP2 at entry *n* of the same list (`PRP_LIST + 8 * n`), and that is its list. With a 128 KiB request and no limit (the pc machine), one command uses 31 entries. With a 16 KiB limit (the q35 machine), the request is eight commands of four pages each, and each uses three entries of its own part of the list.

### Waiting for an answer

```rust
loop {
    let answer = Completion::from_words(self.dma.read_words(at));
    if answer.phase == pair.phase {                 // new
        fence(Ordering::SeqCst);                    // the rest of it (and the data) only now
        pair.head = (pair.head + 1) % pair.entries;
        if pair.head == 0 { pair.phase = !pair.phase; }
        self.bus.write(reg::cq_head(pair.id, self.stride), pair.head);
        if answer.id != id { continue; }            // an older command's: not this one
        return if answer.status == 0 { Ok(..) } else { Err(Failure::Status(answer.status)) };
    }
    if self.bus.read(reg::CSTS) & reg::CSTS_FATAL != 0 { return Err(Failure::Fatal); }
    if self.bus.ticks() >= deadline { return Err(Failure::TimedOut); }
    self.bus.pause();
}
```

Each command gets a new id, and an answer whose id is not the one awaited is consumed and skipped. After a timeout or a fatal status, `transfer` resets the controller (off, on, new queues). The request fails with `IO_ERROR`, and the next one has a chance.

## The Rust in it

- **The same seam as AHCI, a second time.** `Disk<B: Bus>` has the same shape as `ahci::Disk<H: Hba>`. The trait has `read`, `write`, `pause` and `ticks`, plus two **provided methods**, `read64` and `write64`, with bodies in the trait itself. Every implementer gets them for free (two 32-bit accesses, low half first, which NVMe allows). An implementer could override them with a real 64-bit access. Neither `Registers` nor `Sim` needs to.
- **Const generics: `read_words::<N>`.** `Dma::read_words` returns `[u32; N]` for whatever `N` the caller asks for: `read_words::<4>(at)` reads an answer, the 16-byte layout spelled once, as a type. `N` is a compile-time number, so the array lives on the stack and its size is checked by the compiler, not at run time.
- **`const fn` for the register arithmetic.** `reg::sq_tail(queue, stride)` and its siblings are `const fn`. They can be evaluated at compile time where their arguments are constants, as in the tests. They are still ordinary functions at run time, where the stride comes from CAP.
- **An enum for failures.** `Failure::Status(u16)`, `TimedOut` and `Fatal` carry what each case needs, and `match` in the driver makes me handle every one. Adding a fourth case would be a compile error at every `match` until handled.
- **The test simulator borrows itself.** When the driver writes a doorbell, `Sim::write` (holding `borrow_mut()` of the shared `State`) runs the commands right there. It reads them out of the same `Vec` that the driver's `Dma` points into, moves data and posts answers, all inside the one borrow. That works because `State`'s methods take `&mut self` and never reach back through the `Rc`; a second `borrow_mut()` would panic at once.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| The MSI-X table in BAR 0 | The driver gets BAR 0 only up to the first MSI-X structure (table or pending bits) in it, rounded down to a page. The driver checks that its doorbells fit | Handing on the whole BAR: an MSI-X entry is "write this value to this address", so a driver that could edit the table could make the device write anywhere. The kernel programs the table; the driver must not. On QEMU the table starts at 0x2000, and the driver gets exactly 0x2000 |
| Queues | One I/O queue pair, 16 deep, one command at a time | A queue per CPU and many commands in flight: what NVMe is built for, and fs sends one request at a time anyway |
| Transfer size | Split each request into commands of at most MDTS, sharing one PRP list | Refusing controllers with a small MDTS: real ones go as low as 32 or 64 KiB, and the split is a few lines |
| The PRP list | Written once at start, one entry per data page; each command points into it | Building a list per command: same result, more writes, more to get wrong |
| Which namespace | The first active one (CNS 2), else 1 | Several namespaces: fs knows one disk |
| Block size | 512 only; a 4 KiB-formatted namespace is refused | Translating 512-byte sectors onto 4 KiB blocks: the block protocol is built on 512 |
| Taking the controller over | Always off, then on: forgets every old queue | Reusing what is there: the old queues are in memory I can't trust |
| Interrupts | MSI-X (vector 0, for both the admin and the I/O CQ), else MSI, else polled; the same code as AHCI's | — |
| One test script or two | `nvme-test.sh`, beside `ahci-test.sh`, the same shape | One script taking the bus as a parameter: the machines, the expected lines and the mutations differ enough that two plain scripts read better |

## Bugs

None in the driver showed up on QEMU. The first boot printed the controller and namespace line, mounted TaajFS and passed `disk check`. Four bugs were in the tests:
- **The simulator's IDENTIFY namespace returned an empty page** when the controller has no namespace list (pre-1.1). It answered "success" without filling the page, and the driver rightly said "the namespace has no blocks". Now the simulator treats "no list" as "namespace 1 only".
- **The "controller left on" test armed its hang too late.** It made the controller ignore the *next* command after the first driver was gone, so the hang hit the second driver's IDENTIFY instead of a dying driver's command. Now the test rings the I/O doorbell itself, with the hang armed, before the second driver starts.
- **`see:` cannot wait for text with a comma in it.** In the boot script's key list, `see:TEXT` uses commas for spaces, and my "namespace 1, 8192 sectors" became a double space that never matched. Also, `see:` searches the whole log, so the restarted driver's line (the same as the first one's) cannot be waited for anyway. The step now waits for init's "nvme ended with code -1001 - restarting it", which only the restart prints. The new driver's line comes within the same moment, and the check afterwards counts two of them.
- **A flaky anchor, in both disk tests.** One run failed with "fs did not find TaajFS". The log said `e[fs] ready: TaajFS on the disk`: the shell had echoed the first typed key onto the same serial line. `ahci-test.sh` had the same `^` anchor and could have failed the same way. Both now look for the line anywhere.

## How I checked it

On the host:
- `nvme`, 14 tests, against `Sim`, a controller with a disk in a `Vec`:
  - **CC.EN** on checks the admin queues and entry sizes, then sets RDY. Off forgets every queue.
  - **The admin registers** may not move while the controller is on: it panics.
  - **Doorbells** must fall on the stride. A tail doorbell runs every command between the SQ's head and the new tail, reading each out of the driver's memory, and posts answers with the right phase. A head doorbell frees CQ slots, and an answer posted to a full CQ panics.
  - **Create CQ/SQ** checks contiguity, size, the vector and that the CQ exists.
  - **I/O** checks the namespace and MDTS, and moves data through PRP1, PRP2 or a PRP list, page by page.

  The tests:
  - set-up and identity;
  - an interrupt-less I/O queue;
  - transfers of 1, 8, 9, 16, 17 and 256 sectors (PRP2 unused, a page, a list), and the last block;
  - 50 commands: three times round a 16-deep queue, so the phase turns;
  - MDTS of 16 KiB (a list part per command) and of 8 KiB (no list);
  - an error status, then success; a block past the end;
  - a command that never ends: timeout, reset (counted), success;
  - a fatal controller, reset;
  - a controller left on with a command in flight, taken over;
  - namespace selection from a list, without a list, an empty list;
  - refusals: 4 KiB blocks, no NVM command set, MPSMIN above 4 KiB;
  - doorbells 8 bytes apart and 4-deep queues;
  - command and answer layouts, PRPs;
  - IDENTIFY answers, good and bad.
- Kernel: `MsiX::first_in`, with the table and pending bits in the same BAR and in different BARs (191 tests).

By booting (`nvme64-test`, new, in `make test64`; about 50 s alone), two machines at once, with **no virtio disk**:
- **pc** (BIOS), with an NVMe controller added:
  - the kernel's `[pci] 00:04.0 NVMe controller: registers 0x...+0x2000, 148 KiB DMA at 0x..., MSI-X - handed to init` (+0x2000: the BAR cut before the MSI-X table);
  - `namespace 1, 8192 sectors (4 MiB), 128 KiB a command, MSI-X interrupts`;
  - `disk check`, all 9 (256 sectors at once against one by one, and the refusals);
  - `cat welcome`, `cat docs/readme`, `wc long`, `wc` itself loaded from the disk;
  - `write saved kept`, `cp long copy`;
  - `disk stall 3`: init stops nvme after 2 s and starts it again. The new one turns the controller off and on, makes new queues, and `disk read 0`, `disk check` and `cat saved` work through it.
  - On the host afterwards, `saved` holds `kept` and `copy` is byte for byte `long`.
- **q35** (UEFI), the controller with `mdts=2`:
  - `16 KiB a command`;
  - `disk check`, all 9 (every 256-sector read is 8 commands);
  - `cat welcome`, `wc long`, `cp long copy` (written 16 KiB a command, byte for byte `long` on the host), `write saved q35`.

Mutation experiments, on the host (`cargo test` in `nvme/`):

| Mutation | Result |
|---|---|
| M1: the phase never flips | 2 tests fail |
| M2: every command's PRP2 points at the start of the list | 1 fails (the 16 KiB split) |
| M3: LBA format 0 used, not the one FLBAS names | 12 fail (the simulator's format 0 is 4 KiB) |
| M4: a controller that is on is not turned off first | 2 fail ("the admin queues moved while the controller was on") |
| M5: MDTS ignored | 1 fails |
| M6: doorbells always 4 bytes apart | 1 fails |
| M7: the CQ head doorbell never written | 3 fail ("the completion queue is full") |
| M8: no reset after a timeout | at first, **only the fatal-error test failed**: in the simulator a hung command just stays silent, and the next one works anyway. The timeout test now also counts that the controller was turned off and on again, and fails too |

And two on QEMU (`make nvme64-test`):

| Mutation | Result |
|---|---|
| B1 = M5 | caught on q35: QEMU answers every 128 KiB read with status 0x02 (invalid field), and fs finds no disk |
| B2 = M4 | caught on pc: **SeaBIOS leaves the NVMe controller on** after looking for something to boot on it. The driver waits for RDY to go to 0, which never happens, init's watchdog restarts it again and again, and fs finds no disk |

clippy: `nvme` (all targets), the kernel, `user`; clean. `user` builds for AArch64 too; `nvme` is built there, but nothing starts it.

`make test64`: every part passed, the first time, in 833 s, `nvme64-test` among them.

The testing note below was run by scripted boots on a fresh disk (`build/note-nvme.img`, made by the same `run-disk` rule), both parts. Its outputs are copied from those boots.

Not tested:
- **On real hardware.** Only QEMU's NVMe controller has run this driver. A real SSD may take longer to turn off and on (CAP.TO), have a smaller MDTS, or put its MSI-X table elsewhere.
- **MSI and polling.** QEMU's NVMe always has MSI-X, so those paths ran only against the simulator.
- **Errors, timeouts, a fatal controller:** only against the simulator.

## What is still missing

- **No shutdown notification.** A real SSD wants CC.SHN ("shutting down, normal") before power goes, so it can save its mapping tables. Without it, the next power-on may be slower, and the drive counts an "unsafe shutdown". TheTaaJ has no orderly power-off yet. When it does, the disk drivers should get a "flush and shut down" request.
- **No flushing**, as with AHCI and virtio: NVMe's FLUSH command (0x00) should come before and after each TaajFS commit.
- **The watchdog and slow controllers.** A real controller may take several seconds to turn off or on (CAP.TO up to 127.5 s). init's watchdog gives a disk driver 2 s, so on such a controller `nvme` would be killed in the middle of `start`. QEMU's is instant. The watchdog should give a starting driver longer.
- **One queue, one command at a time.** Many queues and deep queues are where NVMe's speed comes from.
- **One namespace, 512-byte blocks only.** Most consumer SSDs ship formatted with 512-byte blocks, but not all.
- **DMA is not confined.** As with every driver here, a broken `nvme` could have the controller write anywhere; an IOMMU would stop it.
- **AArch64 has no PCI yet,** so `nvme` is built there but never runs.

## What I learned

- **The firmware leaves things on, again.** For AHCI, SeaBIOS left a port running. For NVMe, it leaves the whole controller enabled with its own queues (B2). "Off before you touch anything" is the first rule of taking over any DMA device the firmware may have used.
- **Interrupt tables are a security boundary.** An MSI message is just a memory write that the device performs. Whoever can edit the MSI-X table chooses what memory the device writes. Cutting the BAR before the table cost a few lines in the kernel and keeps that power with the kernel.
- **A mutation that survives tells you about the test, not the code.** M8 passed the timeout test because my simulator forgave a missing reset. Any test can only check what it looks at; counting the resets made the test say what I meant it to say.
- **The second driver is cheap when the first one built the seams.** R2's kernel part was one new class, one more `Kind`, and the MSI-X cut. The request loop, the device description, init's wiring and the test shape were all R1's.

## Testing note

**1. The PC, with the disk on NVMe**

```
make run64 RUN_DISK_BUS=nvme RUN_DISK=build/nvme-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:03.0 NVMe controller: registers 0x...+0x2000, 148 KiB DMA at 0x..., MSI-X - handed to init` | the kernel found the controller; +0x2000 is BAR 0 cut short of its MSI-X table |
| `[init] started: registry display fs nvme nic net shell kbd; supervising` | init started `nvme`, not `blk` |
| `[nvme] NVMe 1.4 controller at 00:03.0: QEMU NVMe Ctrl; namespace 1, 32768 sectors (16 MiB), 128 KiB a command, MSI-X interrupts` | the controller's and the namespace's IDENTIFY answers |

In the QEMU window:

| Type | You should see |
|---|---|
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` |
| `write nvme hello` | `nvme: 6 bytes` |
| `cat nvme` | `hello` |
| `wc nvme` | `nvme: 1 line, 1 word, 6 bytes` |
| `disk stall 3` | `disk: the driver will answer nothing for 3 s`; in the log, `[init] nvme has not answered for 2 s - stopping it`, `[init] nvme ended with code -1001 - restarting it`, and the `[nvme] NVMe 1.4 controller ...` line again |
| `disk read 0` | `0000  4a 41 46 53 04 00 00 00 00 10 00 00 00 10 00 00  JAFS............` |
| `cat nvme` | `hello` |

Close QEMU, then on the host:

| Type | You should see |
|---|---|
| `tools/tfstool-rs/target/release/tfstool get build/nvme-disk.img nvme` | `hello` |

**2. A q35 under UEFI, with a controller that moves 16 KiB a command**

```
qemu-system-x86_64 -machine q35 -m 512M -bios /usr/share/ovmf/OVMF.fd -cdrom build/image64.iso -serial stdio \
    -drive file=build/nvme-disk.img,format=raw,if=none,id=n0 -device nvme,serial=taaj0002,drive=n0,mdts=2
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[pci] 00:03.0 NVMe controller: registers 0xc000000000+0x2000, ...` (a 64-bit BAR above 4 GiB) and `[nvme] NVMe 1.4 controller at 00:03.0: QEMU NVMe Ctrl; namespace 1, 32768 sectors (16 MiB), 16 KiB a command, MSI-X interrupts` |
| `cat nvme` | `hello`, written in part 1 |
| `disk` | `disk: 32768 sectors of 512 bytes (16384 KiB)` |
| `disk check` | `disk check: all 9 checks passed` (each 256-sector read is eight commands) |

**3. The tests by themselves**

```
make nvme64-test
make test64
```

| Look for | Meaning |
|---|---|
| `nvme drove the disk on a PC's added NVMe controller (a request a command) and on a q35's (16 KiB a command): ...` | both machines |
| `test64: all passed` | every part |

Automated: make nvme64-test
