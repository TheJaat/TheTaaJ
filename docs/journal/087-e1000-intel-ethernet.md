# Intel Ethernet: an e1000 driver, beside nic

*Journal entry #87 · Era 7, step R5. TheTaaJ now drives Intel's gigabit Ethernet cards of the e1000 family: the 82540EM that QEMU, VirtualBox and VMware give a guest by default, and the 82574L. Like the NVMe and AHCI drivers, it is a ring-3 program, `e1000`, with all its logic in a crate tested against a simulated card. The network stack cannot tell it from the virtio driver. That is because the part of `nic` that serves clients is now `taaj::nic_service`, shared by both drivers, as `taaj::block_service` is shared by the three disk drivers.*

## Why

Until now TheTaaJ had a network only inside QEMU with a virtio card: a card that exists only in virtual machines. Real hardware has real cards. The e1000 is the classic first real one: it is what most hypervisors emulate when nothing is said, and its register set and descriptors live on in Intel's later cards, the 82574L among them. A driver for it is also a step towards R7: it is the first network driver that talks to a card the way real silicon is talked to, with descriptor rings in memory and registers behind a memory BAR.

## What it is

```
 kernel: devices.rs              init                       e1000 (user/src/bin)           net, niccheck, ...
 ------------------              ----                       --------------------           ------------------
 8086:100E/100F/1004/10D3  ─►    card_driver = "e1000"  ─►  e1000 crate: Card::start   ─►  abi::nic, unchanged
 BAR 0 (memory), DMA,            same channels as nic       taaj::nic_service::serve
 MSI (never MSI-X) or none
```

- **`e1000/` crate (TheTaaJ, beside `nvme/`):**
  - `reg`: the registers;
  - `desc`: the 16-byte descriptors;
  - `layout`: where the rings and buffers lie in the DMA memory;
  - `Card<B: Bus>`: `start`, `receive`, `send`, `take_sent`, `transmit_full`, `enable_interrupts`, `interrupt_seen` and `link_up`.

  10 host tests against `Sim`, a simulated card.
- **`taaj::nic_service` (user/src/nic_service.rs):**
  - who listens for which kind of frame;
  - passing frames on in pieces;
  - assembling frames to send;
  - INFO, STATS and the watchdog's STALL;
  - waiting on an interrupt, or polling.

  It is moved out of `nic.rs` nearly line for line. A driver gives it a `nic_service::Card`: seven methods.
- **`nic`** keeps only the virtio card: queues, buffers, its two ways of being reached (ports on a PC, memory on AArch64).
- **`e1000`** (program): maps BAR 0 and the DMA memory, starts the card, and calls `nic_service::serve(card, "e1000", ...)`.
- **Kernel:** `Kind::Intel` in `devices.rs`. The card is recognised by vendor and device number, since Intel's Ethernet class code is shared with every other Ethernet card. It gets BAR 0, 104 KiB of DMA memory, and MSI if the card has it.
- **init:** takes a virtio card or an Intel one (the first on the bus wins). It starts `nic` or `e1000` on the same channels, and stops whichever it is during `poweroff`.

## How an e1000 sends and receives

Where a virtio card has a "virtqueue", an e1000 has a **descriptor ring** for each direction: an array of 16-byte descriptors in memory, each naming one buffer, and two index registers, *head* (the card's) and *tail* (the driver's).

```
 receive ring (32)                         transmit ring (16)
 ┌──┬──┬──┬──┬──┬──┬──┬──┐                  ┌──┬──┬──┬──┬──┬──┐
 │DD│DD│  │  │  │  │  │  │                  │DD│  │  │  │  │  │
 └──┴──┴──┴──┴──┴──┴──┴──┘                  └──┴──┴──┴──┴──┴──┘
  ↑rx_next  ↑RDH (card fills here)      ↑     ↑tx_clean   ↑TDH ↑TDT=tx_tail
                         RDT (last given)┘     (oldest)   (card sends from here)
```

- **Receive.** The descriptors from RDH up to RDT are empty buffers the card may fill. It writes a frame into the buffer at RDH, the length and "done" (DD) into the descriptor, and moves RDH on. The driver keeps its own `rx_next`. Each descriptor there with DD set holds a frame: the driver takes it, clears DD, and gives the descriptor back by writing its index to RDT.
- **Transmit.** The driver fills the descriptor at TDT (`tx_tail`) with the buffer's address, the length and the command (end of packet, insert the CRC, report status) and moves TDT on. The card sends from TDH to TDT and writes DD into each descriptor it has finished. The driver reclaims from `tx_clean` while DD is set.
- **One descriptor is always left empty.** Head equal to tail must mean "the card has none", so the ring is full one short of its size. At start RDT is 31 of 32, not 32.

Setting it up (`Card::start`), in the manual's order:
1. **Reset** (CTRL.RST), and wait until the bit clears itself. This makes the card forget everything an earlier driver gave it, which is what makes init's restart of a hung driver safe.
2. **Mask every interrupt** (IMC) and read ICR to clear what was pending.
3. **Read the address** from RAL0/RAH0, which the reset loaded from the card's EEPROM. Refuse a card whose "address valid" bit is clear.
4. **Link up** (CTRL.SLU) with speed detection, and clear the bits a driver must leave clear: PHY reset, link reset, VLAN mode, inverted loss of signal. Empty the multicast table.
5. **The receive ring.** Every descriptor names its buffer, with status cleared. Then RDBAL/RDBAH, RDLEN, RDH = 0, RDT = 31, and the receiver on: RCTL with EN, broadcasts accepted (BAM), CRC stripped (SECRC), 2048-byte buffers.
6. **The transmit ring:** TDBAL/TDBAH, TDLEN, TDH = TDT = 0, the inter-frame gaps (TIPG), and the transmitter on: TCTL with EN, short frames padded (PSP), the collision values the manual gives for full duplex.

A difference from virtio that changed a test: **the card filters by address itself.** A virtio card in QEMU hands the driver every frame on the wire, and the driver drops those for others, counting them as `not_ours`. An e1000 compares the destination with RAL0/RAH0 and drops strangers before they reach memory. niccheck's "frames for others are kept back" check expected the driver to count two. It now accepts exactly either "2 counted of 3 seen" (virtio) or "none counted, 1 seen" (Intel). Both prove that only the broadcast was passed on.

## Interrupts: MSI or nothing

- **The 82540EM has no MSI.** It is a plain PCI card with an interrupt *pin*. Which interrupt line that pin reaches is written in the ACPI tables as AML (`_PRT`), which TheTaaJ does not run. So the driver polls: the service looks at the rings every tick (10 ms). A ping says "time under 10 ms" anyway, and niccheck's timing checks pass.
- **The 82574L has both MSI and MSI-X.** The kernel deliberately gives it **MSI only**. In MSI-X mode the card sends each cause (receive, transmit, link) to a vector chosen through a register called IVAR, which this driver does not program. I tried it (mutation B3 below): with MSI-X turned on, the interrupts never arrive and niccheck fails. With MSI, the card behaves as the 82540EM, with one interrupt for everything, and reading ICR is the acknowledgement.

The interrupt causes the driver unmasks: frame received (RXT0), receive descriptors low (RXDMT0), receive overrun (RXO), link change (LSC), and descriptor written back (TXDW). The last one matters when the transmit ring is full: the service waits on the interrupt alone, and TXDW is what says room has come.

## The Rust in it

- **A trait object, `&mut dyn FnMut(Option<&[u8]>)`.** `nic_service::Card::receive` takes the function to call for each frame as `&mut dyn FnMut`. `dyn` means the exact closure type is not known at compile time: the call goes through a table of function pointers. That makes `Card` usable as a trait with different drivers behind it. (The crate's own `Card::receive` takes `impl FnMut`, a generic: one copy of the function per closure type, and no table.) Both work. The difference is what is decided at compile time.
- **Disjoint borrows.** `card.receive(&mut |frame| service.arrived(frame, &address))` borrows `card` mutably *and* `service` mutably at the same time. Rust allows it because they are two different variables. The same reasoning lets `Service::piece` hold `whole` (a borrow of `self.outgoing`) while adding to `self.stats.sent`: different fields of one struct.
- **Calling an inherent method of the same name.** In `nic.rs`, `impl nic_service::Card for Card { fn send(...) { Card::send(self, frame) } }`. Inside the trait implementation, `self.send(...)` could be read as a recursive call to the trait's own `send`. `Card::send(self, ...)` names the struct's own method. (Rust prefers inherent methods anyway; naming it says so out loud.)
- **Destructuring in a parameter.** `fn shut_down(..., [disk_driver, card_driver]: [&str; 2])`: the array is taken apart in the parameter list itself, so the body uses two plain names.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where the client half lives | `taaj::nic_service`, shared by `nic` and `e1000` | Copying ~250 lines into `e1000.rs`: two copies of the listener and STALL logic would drift apart. `block_service` already set the pattern |
| How cards are recognised | Vendor 8086, four device numbers (`e1000::MODELS`) | By class (02.00 Ethernet): every vendor's card has it, and each speaks its own registers |
| Which models | 82540EM, 82545EM, 82543GC, 82574L: the ones QEMU has | Intel's I217/I218/I219 (most PCs since 2013): "e1000e" by name, but a different PHY interface and power management. A later driver, once the real PC (R7) shows which one it has |
| Interrupt | MSI, else polled; never MSI-X | MSI-X with IVAR set up: more code for one model, and nothing to gain with one queue. Pin interrupts: they need `_PRT` from AML |
| Address | RAL0/RAH0 after reset | Reading the EEPROM through EERD: its format differs between models (82540 and 82574), and the reset already copies the address into RAL0/RAH0 on every model here |
| Descriptors | Legacy (16 bytes), one buffer per frame | Extended descriptors (checksum offload, split headers): the network stack computes its own checksums |
| Ring sizes | 32 receive, 16 transmit, 2048-byte buffers, 104 KiB of DMA | The virtio driver's numbers are similar; nothing measured asks for more |
| Bound on `receive` | At most one ring's worth per call | An unbounded loop: a card that keeps marking descriptors done (or a bug like mutation M2) would keep the driver there for ever and starve its clients |

## Bugs

- **`AccessDenied` mapping the registers.** The first boot said `[e1000] cannot map the card's registers: AccessDenied`, five times, and init gave up. init grants the card's register handle to its driver with READ|WRITE only for a virtio-mmio card, because a virtio PCI card's registers are I/O ports, which need no map rights. An Intel card is PCI *and* memory. The disk driver's grant already asked `info.registers_in_memory()`; the card's now does too.
- **The kernel image got too close to its limit (from R3).** The big-kernel test broke in R3 for this reason, and every embedded program adds to it. e1000 adds about 25 KB; the kernel now ends at about 2.28 MiB, and the BIOS loader's limit is 3 MiB. USB will need room. Recorded below.
- **Test simulator mistakes, mine:**
  - the first `Sim` refused any write to TDT before the transmitter was on, but the driver's `TDT = 0` at setup hands over nothing, which is legal. The check now applies only when there is something to send;
  - mutation M2 hung the test instead of failing it. That showed the driver's receive loop had no bound, which became a design decision (above).

## How I checked it

On the host, `e1000`: 10 tests against `Sim`.
- `Sim` resets the way the manual describes: registers cleared, the address reloaded from its "EEPROM", the RST bit clearing itself after a delay.
- It sends what the transmit descriptors between head and tail name, checking:
  - EOP, IFCS and RS are set;
  - the length is between 60 and 1514;
  - the transmitter is on.
- It filters arriving frames by address, as the card does. It fills receive buffers, or misses a frame when no descriptor is free.
- It splits frames too big for a buffer over several descriptors.
- It refuses ring lengths that are not multiples of 128, and tails past the ring.

The tests:
- start: one reset; the address; interrupts masked; link up; RCTL and TCTL; both rings' places; RDT = 31; the multicast table empty;
- an empty EEPROM, and a reset that never ends, refused;
- 100 frames in five rounds of 20 (three times round the ring), each whole and in order; broadcasts in, strangers out;
- 40 frames with nobody looking: 31 taken, 9 missed, then every buffer usable again;
- a damaged frame and an oversized one dropped whole, the next one fine;
- 51 frames out in order, a 20-byte one padded to 60 with zeroes, one too big refused;
- a busy card: 15 frames fill the ring, the 16th waits, then goes;
- reading ICR clears it;
- a second driver after the first: a second reset, the old frames forgotten, the new rings working.

Mutation experiments, one at a time:

| Mutation | Result |
|---|---|
| M1: no receive descriptor given at start (RDT = 0, equal to the head) | 6 tests fail |
| M2: a taken descriptor's status not cleared | at first, the test **hung** (stale DD bits read for ever); after the bound on `receive`, 2 tests fail |
| M3: no RS (no status written back) | 4 fail (the simulator refuses it) |
| M4: no reset | 9 fail |
| M5: the last piece of an oversized frame passed on | 1 fails |
| M6: short frames not padded | 1 fails |
| M7: the ring's last descriptor used too | 1 fails |
| M8: broadcasts not accepted (no BAM) | 2 fail |
| M9: receive errors ignored | 1 fails |
| M10: the CRC kept (no SECRC) | 5 fail |
| M11: interrupts not masked at start | **survived**: the reset already masks them all, in the manual and in the simulator. The IMC write is belt and braces, and no test can tell |
| M12: a transmit descriptor reclaimed before DD | 1 fails |

By booting (`make e1000-64-test`, new, in `make test64`): the network tests from entries 062-064, rerun over Intel cards.
- **niccheck, 82540EM (polled):**
  - 233 checks: echoes, bursts of 20 and 100, long frames, frames for others, careless clients;
  - the host's end of the wire counts every frame;
  - then the driver made to hang, stopped by init, restarted, and working again;
  - then the cable going nowhere.
- **niccheck, 82574L (MSI):** the same.
- **IP over the 82540EM** (ip-test.sh): ARP, pings, UDP, against the hand-made router and QEMU's network; the driver replaced in the middle, and the network service notices.
- **TCP over the 82574L** (tcp-test.sh): three connections to the host, 300,029 bytes.

`net64-test` and `aarch64-net-test` still pass: `nic`, now on `nic_service`, on a PC and on AArch64.

Mutations on QEMU:

| Mutation | Result |
|---|---|
| B2: the driver never unmasks interrupts (82574L) | caught: "eleven frames out and their answers back take less than a second" fails. The frames were only seen when a client's request woke the driver |
| B3: the kernel turns MSI-X on for the 82574L | caught, the same way: no interrupt ever comes. This is the reason for MSI only |

clippy: `e1000` (all targets), `user` (PC and AArch64), the kernel: clean.

`make test64`: all passed, in 14 minutes 29 seconds (with the new `e1000-64-test` among its parts).

The testing note below was run by scripted boots on a fresh disk (`build/note-e1000.img`, made by the same `run-disk` rule): netinfo, three pings and poweroff with the 82540EM; netinfo and three pings with the 82574L. Its outputs are copied from those boots.

Not tested:
- **On real hardware.** Only QEMU's models of these cards. A real 82540EM's PHY takes time to bring the link up; this driver reads the link once at start and on INFO, so a link that comes up late is reported "down" in the boot line, but frames flow once it is up.
- **Link changes.** The link-change interrupt is unmasked and acknowledged, but nothing tells the network service.

## What is still missing

- **Intel's newer onboard cards (I217/I218/I219/I225).** They are probably what the real PC has, and they are not driven. R7 will show which one it is.
- **Pin interrupts**, for the 82540EM and every other card without MSI: they need the ACPI `_PRT` table (AML), and so an AML interpreter.
- **Link state**: watched, not reported. With a cable unplugged and plugged in again, the network service would not know.
- **Offloads:** checksums, segmentation, VLAN tags. The CPU does all of it.
- **Statistics registers** (the card's own counts of CRC errors, collisions, missed frames): not read.
- **Kernel image size:** about 735 KiB left below the BIOS loader's 3 MiB.

## What I learned

- **The second driver is where an interface proves itself.** `abi::nic` was written with one card in mind. Splitting `nic.rs` in two showed that the protocol was already card-free, and showed exactly what a card has to do: seven methods. The only thing the second card changed was one test assumption (who filters addresses).
- **A test's assumption can be a fact about the hardware, not about correctness.** "Two frames for others were counted" was true of virtio and false of Intel, while both were correct. The check now states the real property: nothing but the broadcast was passed on.
- **A mutation that hangs is a finding.** The test hung instead of failing, and that showed the driver trusted the card to stop marking descriptors done. Now a card that misbehaves costs at most one ring's worth of work per look.
- **MSI-X is not "better MSI" for free.** It moves the choice of vector into the device, and a device you have not told what to choose sends nothing. MSI was the honest choice for one queue.

## Testing note

**1. The 82540EM (QEMU's `e1000`): polled**

```
make run64 RUN_NIC=e1000 RUN_DISK=build/e1000-disk.img
```

A new 16 MiB disk is made the first time (your `build/taaj-disk.img` is not touched). In the terminal's log:

| Look for | Meaning |
|---|---|
| `[pci] 00:04.0 Intel Ethernet card: registers 0xfeb80000+0x20000, 104 KiB DMA at 0x..., no interrupt (polled) - handed to init` | the kernel found the card and handed it on, without an interrupt (it has no MSI) |
| `[init] started: registry display fs blk e1000 net shell kbd power; supervising` | init chose `e1000`, not `nic` |
| `[e1000] Intel 82540EM at 00:04.0: address 52:54:00:12:34:56, link up, rings of 32 and 16, polled` | the driver reset the card and read its address |
| `[net] this machine is 10.0.2.15/24 on card 52:54:00:12:34:56, with its router at 10.0.2.2` | the network service asked the new driver who it is, as it asks `nic` |

In the QEMU window:

| Type | You should see |
|---|---|
| `netinfo` | `address  10.0.2.15/24`, `router   10.0.2.2`, `card     52:54:00:12:34:56` |
| `ping 10.0.2.2 3` | three lines `56 bytes from 10.0.2.2: seq=N ttl=255 time under 10 ms`, then `3 sent to 10.0.2.2, 3 answered` |
| `poweroff` | `Switching off...`; in the log `[init] stopped net`, then `[init] stopped e1000` (the card's driver, in nic's place), `fs`, `blk`, ..., and QEMU ends by itself |

**2. The 82574L (QEMU's `e1000e`): MSI**

```
make run64 RUN_NIC=e1000e RUN_DISK=build/e1000-disk.img
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[pci] 00:04.0 Intel Ethernet card: registers 0xfeb80000+0x20000, 104 KiB DMA at 0x..., MSI - handed to init` |
| (the log) | `[e1000] Intel 82574L at 00:04.0: address 52:54:00:12:34:56, link up, rings of 32 and 16, MSI interrupts` |
| `ping 10.0.2.2 3` | `3 sent to 10.0.2.2, 3 answered` |

**3. The virtio card, as before**

```
make run64 RUN_DISK=build/e1000-disk.img
```

| Look for / Type | You should see |
|---|---|
| (the log) | `[nic] virtio network card at 00:04.0: ...` and `nic` (not e1000) in init's started line |
| `ping 10.0.2.2 3` | `3 sent to 10.0.2.2, 3 answered` |

**4. The tests by themselves**

```
make e1000-64-test
make net64-test
make test64
```

| Look for | Meaning |
|---|---|
| `wire: ok: 39 frames from 52:54:00:12:34:56: ...` (twice) | niccheck over the 82540EM and the 82574L, counted by the host's end of the wire |
| `host: ok: 5 checks; 3 connections from the machine served, 300029 bytes` | TCP over the 82574L |
| `e1000-64-test passed` | all four |
| `test64: all passed` | every part |

Automated: make e1000-64-test
