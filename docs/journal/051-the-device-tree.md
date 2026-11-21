# The Device Tree: A Machine Describes Itself

*Journal entry #51 · Era 3, step 3b. The AArch64 kernel stops assuming what machine it is on. It reads the device tree the loader hands it - memory, CPUs, the serial port, the interrupt controller, the timer, how to call the firmware - and the serial port it prints through is now the one the tree names, not a number I typed.*

```
[tree] device tree at 0x48000000: 1048576 bytes, version 17, 62 nodes, the deepest 5 down
[tree] machine     "linux,dummy-virt"
[tree] memory      0x40000000 - 0x1c0000000 (6144 MiB)
[tree] cpus        4, with ids [0, 1, 2, 3]
[tree] serial      PL011 at 0x9000000, named as the console
[tree]             its clock runs at 24000000 Hz
[tree] interrupts  GIC version 3: shared part at 0x8000000, per-CPU part at 0x80a0000
[tree] timer       interrupts 29 (secure), 30 (physical), 27 (virtual), 26 (hypervisor)
[tree] firmware    called with smc
```

---

## Why

Entry 50 ended with this in the code:

```rust
const QEMU_VIRT_UART: u64 = 0x0900_0000;
```

One number, right on one machine. The next steps need more of the same kind: where memory is (step 3c), where the interrupt controller is and which version (3d), how many CPUs and how to start them (3h). I could type each of those in for QEMU's `virt` machine. Then the kernel would run on QEMU's `virt` machine.

On a PC this problem is answered by ACPI (entry 6): the firmware leaves tables in memory. Most AArch64 boards, and all RISC-V ones, answer it another way. The loader gives the kernel the address of one block of bytes that describes the whole machine: the **device tree**.

## What a device tree is

A tree of **nodes**. Each node has a name and **properties**; a property is a name and some bytes. Written as text, the part of QEMU's that this entry uses looks like this:

```
/ {
    #address-cells = <2>;
    #size-cells = <2>;
    model = "linux,dummy-virt";

    memory@40000000 {
        device_type = "memory";
        reg = <0x0 0x40000000  0x0 0x20000000>;
    };

    pl011@9000000 {
        compatible = "arm,pl011", "arm,primecell";
        reg = <0x0 0x9000000  0x0 0x1000>;
        clocks = <0x8000 0x8000>;
    };

    apb-pclk {
        phandle = <0x8000>;
        clock-frequency = <24000000>;
    };

    cpus {
        #address-cells = <1>;
        #size-cells = <0>;
        cpu@0 { device_type = "cpu"; reg = <0>; };
    };

    chosen {
        stdout-path = "/pl011@9000000";
    };
};
```

Four conventions carry nearly everything.

**`compatible`: what a device is.** A list of names, from the exact part to more general ones that it can be driven as. A driver looks for a name it knows. I look for `arm,pl011`.

**`reg`: where it is.** Pairs of (address, size). But a property is only bytes - how many bytes make an address? The *parent* says: `#address-cells` and `#size-cells` count 32-bit numbers ("cells"). At the top of this tree both are 2, so memory's `reg` is four cells: address `0x0_40000000`, size `0x0_20000000`. Under `cpus` they are 1 and 0: a CPU has a number and no size.

**`phandle`: pointing at another node.** The serial port's clock is not a number in the serial port's node. It is `clocks = <0x8000>`, and somewhere else is a node with `phandle = <0x8000>`. A phandle is a pointer, written as a number that the two nodes share.

**`/chosen`: the loader's own notes.** `stdout-path` is "this is the console".

## The bytes

The text above is for people. What the kernel gets is the *flattened* form (hence "FDT", "DTB"):

```
+-------------------+
| header            |  ten numbers: sizes, and where the parts are
+-------------------+
| reserved memory   |  (address, size) pairs the kernel must leave alone; ends with (0, 0)
+-------------------+
| structure         |  the tree, as a stream of tokens
+-------------------+
| strings           |  every property name, once, each ended by a zero byte
+-------------------+
```

The structure is the tree in the order you would read it aloud:

```
BEGIN_NODE ""                      the root
  PROP  len=4  name@0    00 00 00 02          #address-cells = <2>
  BEGIN_NODE "memory@40000000"
    PROP  len=7   name@27   "memory\0"        device_type
    PROP  len=16  name@39   00 00 00 00 40 00 00 00 ...   reg
  END_NODE
  ...
END_NODE
END
```

Three details:

- every token starts on a multiple of four bytes, so names and values are padded;
- a property's name is not written in place: it is an offset into the strings part, so `reg`, which nearly every node has, is stored once;
- **every number is big-endian**, most significant byte first, on every machine. The device tree was born on PowerPC. `u32::from_be_bytes` is the whole cost.

## The reader

`kernel64::devicetree`, in the kernel's library, so it is compiled and tested on my laptop. Two promises shaped it.

### It allocates nothing

The kernel reads the tree to find out where memory is. It cannot use a heap to do that. So there is no "parse into a tree of structs" step. A `Node` is little more than a position - *where in the blob this node's properties begin* - and everything it returns is a slice of the blob:

```rust
pub struct Node<'a> {
    tree: Tree<'a>,
    pub name: &'a str,        // a slice of the blob
    pub depth: usize,
    offset: usize,            // where its properties begin
    address_cells: u32,       // how to read its reg: said by its parent
    size_cells: u32,
    pub direct: bool,
}
```

`'a` is a **lifetime**: the compiler's name for "as long as the blob is borrowed". `&'a str` inside `Node<'a>` says the name lives exactly as long as the blob the tree was made from. I cannot keep a node's name after the blob is gone; the compiler would refuse the program.

Walking is done by **iterators** - values with a `next()` that the `for` loop calls:

```rust
for node in tree.nodes() { ... }             // every node, parents first
for child in node.children() { ... }         // one level down
for (name, bytes) in node.properties() { ... }
for range in node.reg() { ... }              // (address, size) pairs
```

Each is a small struct holding an offset. Asking for a node's property walks its tokens again from the start. That is slower than a table, and there are 62 nodes.

The one thing the walk must *remember* is the cell counts of each ancestor, to read `reg` correctly. With no heap that is a fixed array, so there is a limit: sixteen levels. QEMU's tree is five deep. Deeper nodes are passed over, not a crash.

### It trusts nothing

The loader is not an attacker. But the address in `x0` may be wrong, and a kernel that crashes while reading its own description says nothing useful - the console is not even set up yet. So no length in the blob is used to index anything directly:

```rust
/// The 32-bit big-endian number at `offset`, if it is inside `bytes`.
fn number(bytes: &[u8], offset: usize) -> Option<u32> {
    let four = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([four[0], four[1], four[2], four[3]]))
}
```

`bytes.get(range)` returns `None` where `bytes[range]` would panic. `checked_add` returns `None` where `+` would overflow. The `?` after each means "if this is `None`, return `None` from the whole function now". A length that runs off the end, a name with no zero byte, a tree that never closes: each becomes "there is nothing more here".

### What the kernel asks of it

`Machine::read(&tree)` makes one pass and fills a fixed-size summary:

| Wanted | Found by |
|---|---|
| memory | nodes with `device_type = "memory"`: their `reg` |
| CPUs | nodes with `device_type = "cpu"`: their `reg` is the CPU's identifier |
| serial port | the node `/chosen`'s `stdout-path` names, if it is an `arm,pl011`; else the first `arm,pl011` |
| …its clock | the first number in its `clocks` is a phandle; that node's `clock-frequency` |
| interrupt controller | `compatible` with `arm,gic-v3` (version 3) or `arm,cortex-a15-gic` and its relatives (version 2); two `reg` pairs |
| timer | `arm,armv8-timer`: its `interrupts`, three cells each |
| firmware calls | `arm,psci`: its `method`, `"hvc"` or `"smc"` |

Nodes that say `status = "disabled"` are not used. A board's tree often lists every device the chip has, wired up or not.

### One thing it does not do

A bus can number its devices its own way. Its `ranges` property says how the bus's numbers become its parent's; an empty `ranges` means "the same". Translating through a non-empty `ranges` is not written yet. What is written is knowing *when it would be needed*: every node carries `direct`, true only if every bus above it says "the same". The summary takes addresses only from direct nodes. A device behind a renumbering bus is not found, rather than found at a wrong address.

QEMU's `virt` needs no translation. A Raspberry Pi does (everything is under `/soc` with a real `ranges`), so this has a date.

## In the kernel

The order of `kernel_main` changed in a way that says what the tree is for:

```rust
// The device tree first, and silently: it says where the serial
// port is, so nothing can be said until it has been read.
let tree = arch::device_tree(device_tree).map(Tree::new);
let machine = match &tree {
    Some(Ok(tree)) => Machine::read(tree),
    _ => Machine::default(),
};
let console_from_tree = arch::init_early_console(machine.serial);

kprintln!("Hello from TheTaaJ!");
```

The whole tree is read before the first character is printed.

And if there is no tree, or it names no serial port? Then there is nowhere to say so. The constant from entry 50 is still there, as the last resort and under an honest name: with it the kernel can at least print *"no serial port with a clock described: using the one QEMU's virt machine has, as a guess"*.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Parse into what | Nothing: nodes are positions in the blob | *A tree of structs with `Vec`s.* Needs the heap, which needs memory, which is what the tree is read to find. |
| Deep trees | A fixed limit of 16 levels; deeper nodes skipped | *Recursion.* The depth of the kernel's stack would then be chosen by whoever wrote the blob. |
| A malformed blob | Every read is checked; the walk just ends | *Validate once, then trust.* Two passes, and the second has to stay in step with the first for ever. |
| Bus address translation | Not done; nodes that would need it are marked and not used | *Assume addresses are the CPU's.* True on QEMU and silently wrong on the first real board. |
| No tree, or no serial port | Fall back to QEMU `virt`'s address, and say so | *Stay silent.* Correct in principle, and useless: the one case where I most need a message would have none. |
| Where the summary lives | The library (`Machine`), not `arch/aarch64` | RISC-V uses the same trees; and it can be tested on the host against real blobs. |
| Test data | Two real trees from QEMU in the repository (7.5 and 8 KiB), and a builder for made-up ones | *Only made-up trees.* I would be testing my reading of the specification against my writing of it. |
| Interrupt numbers | Assume three cells each, as the GIC defines | *Read `#interrupt-cells` from the interrupt parent.* Right, and needed when there is a second kind of controller. |

## Bugs

**Every node was "disabled".** The first run of the tests found no memory at all. A text property is read by cutting the value at zero bytes; for a property that is *missing* I started from an empty value, and cutting an empty value gives one empty text. So `node.text("status")` was `Some("")` for every node that has no `status` - not "okay", so not used. Fixed where it belonged: a value that does not end in a zero byte holds no texts.

**A disabled serial port could be chosen.** I found this one by breaking the code on purpose. I made the summary ignore `status` altogether, to see a test fail - and none did. Looking for why, I saw that the search for a serial port never looked at `status` in the first place. The check was in the loop and not in the search. Now both call `node.enabled()`, and a tree whose *first* serial port is disabled is a test.

**A test of mine that tested nothing.** One of the deliberate breakages was no change at all: I replaced `<< 32` on a 64-bit number with a "wrapping" shift, which for 32 is the same thing. All tests passed, correctly. It still pointed at a gap - nothing read an address of three cells - so there is a test for that now.

## How I checked it

**Automated** (`make test64`):

- **Host tests**: 141 in the kernel's library, 22 of them for the device tree:
  - the walk's order and depths; properties as numbers, texts, lists; `reg` with 2+2, 1+1, 1+0 and 3+2 cells, and the defaults when a parent does not say;
  - paths with and without `@address`; aliases; an alias that names itself; phandles;
  - `direct`: under the root, under a bus with empty `ranges`, with real `ranges`, with none;
  - disabled nodes; more memory ranges than there is room for; a tree twenty levels deep;
  - **two real trees from QEMU 8.2**: one CPU, 512 MiB, GIC version 2, `hvc`; and four CPUs, 2 GiB, GIC version 3, `smc`;
  - **damage**: the real tree cut short at every one of its 7,510 lengths, and 4,000 copies with one to four bytes changed at random (from a fixed seed) - walked in full each time. None may panic.
- **Boots** (`make aarch64-test`): every boot now also checks the `[tree]` lines against what that machine was started with:

| Boot | Must read from the tree |
|---|---|
| `plain`, `dirty` | 512 MiB, 1 CPU, GIC 2, `hvc` |
| `el2` | …`smc`: with exception level 2 present, QEMU moves the firmware up a level |
| `a53` | **6144 MiB**: memory ends above 4 GiB, which needs both cells of a size |
| `max` | 128 MiB, GIC **3** |
| `smp4` | **4** CPUs, ids 0 to 3 |
| `unnamed` | a tree with `stdout-path` taken out: `the first one found` |
| `clockless` | a tree with the serial port's `clocks` taken out: `its clock is not given`, and the line that says the kernel guessed |

  The last two use a new script, `dtb-without.py`: QEMU writes out the tree it would have made, the script overwrites one property with "no operation" tokens, and QEMU passes the result on.

**By hand - breaking it on purpose** (five changes to the reader, each put back):

| Change | Result |
|---|---|
| children read `reg` with the defaults, not their parent's counts | 4 tests fail |
| the depth bookkeeping never comes back up | 1 test fails |
| a property's length is used to index directly | the damage test panics, as it should |
| `status` is ignored | **no test failed** - see Bugs; now 1 fails |
| the shift | not a change - see Bugs |

**Not tested:**

- **That the address in the tree is the one used.** QEMU's serial port cannot be moved, so a kernel that ignored the tree and used the old constant would pass every boot. Only the *clock* path shows a difference (`clockless`).
- **No tree at all**, and a tree that fails its header check, in a boot. QEMU always supplies a good one. Both paths are a few lines and are covered by the host tests of the header; the lines that print the message have never run.
- **A tree in a different GiB from the kernel**, or crossing a GiB boundary (refused).
- **Any tree but QEMU's.** A Raspberry Pi's has `ranges` to translate, a different serial port for its console, and `/memreserve/` entries. The reserved list is read and printed; it has only ever been empty.

## What is still missing

- **`ranges` translation.** Above.
- **Interrupts of ordinary devices** (`interrupts`, `interrupt-parent`, `interrupt-map`). Only the timer's are read.
- **`/chosen/bootargs`** (a command line) and the initial ramdisk's address.
- **`/reserved-memory`** nodes: a second, newer way to reserve memory, besides the list in the header.
- **Nothing is done with most of it yet.** Memory is printed, not managed; the interrupt controller is found, not set up. That is steps 3c and 3d.
- **ACPI on AArch64.** Servers use it instead of a device tree. Not planned.

## What I learned

- **A device tree is ACPI's job done the other way round.** ACPI: the firmware is asked questions and partly runs code to answer. Device tree: the loader hands over a finished document. The second is far easier to read, and cannot describe anything that changes.
- **The parent says how to read the child.** `#address-cells` is the oddest idea in the format and the one everything else rests on. A value has no type; its context gives it one.
- **Lifetimes, used rather than fought.** `Node<'a>` borrowing from the blob is the natural way to write a reader with no allocator, and the compiler checks the one thing that could go wrong: using a slice after its memory is gone.
- **`get` and `?` instead of `[]`.** A parser of untrusted bytes written with `Option` everywhere reads almost the same as one written with indexing, and the difference is that it cannot crash. The damage test found nothing to fix - which, having seen it catch the version that indexes, I believe.
- **Break it to find what is not tested - and sometimes what is not written.** I went looking for a missing test and found a missing check.
- **A fallback must be able to announce itself.** The guess for the serial port exists so that the kernel can say that it is guessing.

## Testing note

**1. The ordinary machine**

```
make run-aarch64
```

Nothing to type. To leave QEMU: **Ctrl+A, then X**.

| Look for | Meaning |
|---|---|
| `Hello from TheTaaJ!` and the `[cpu]` lines, as in entry 50 | the serial port now comes from the tree, and still works |
| `[tree] device tree at 0x48000000: 1048576 bytes, version 17, 56 nodes, the deepest 5 down` | the whole tree was walked |
| `[tree] machine     "linux,dummy-virt"` | QEMU's name for its machine |
| `[tree] memory      0x40000000 - 0x60000000 (512 MiB)` | |
| `[tree] cpus        1, with ids [0]` | |
| `[tree] serial      PL011 at 0x9000000, named as the console` | found through `/chosen` |
| `[tree]             its clock runs at 24000000 Hz` | found through a phandle |
| `[tree] interrupts  GIC version 2: shared part at 0x8000000, per-CPU part at 0x8010000` | |
| `[tree] timer       interrupts 29 (secure), 30 (physical), 27 (virtual), 26 (hypervisor)` | |
| `[tree] firmware    called with hvc` | |

**2. A different machine, the same kernel**

```
make run-aarch64 RUN_A64="-M virt,gic-version=3,virtualization=on -smp 4 -m 6G"
```

| Look for | Meaning |
|---|---|
| `…the loader entered at level 2` | |
| `[tree] memory      0x40000000 - 0x1c0000000 (6144 MiB)` | an end above 4 GiB |
| `[tree] cpus        4, with ids [0, 1, 2, 3]` | |
| `[tree] interrupts  GIC version 3: shared part at 0x8000000, per-CPU part at 0x80a0000` | |
| `[tree] firmware    called with smc` | |

**3. A tree with something missing**

```
make aarch64-test
make run-aarch64 RUN_A64="-dtb build/aarch64/clockless.dtb"
```

The first command runs every automated boot (it should end `aarch64-test: all passed`) and leaves the cut-down trees in `build/aarch64/`.

| Look for | Meaning |
|---|---|
| `[tree]             its clock is not given` | |
| `[tree] no serial port with a clock described: using the one QEMU's virt machine has, as a guess` | the fallback, saying that it is one |

and with `unnamed.dtb` in place of `clockless.dtb`: `[tree] serial      PL011 at 0x9000000, the first one found`.

**4. Host checks**

```
cd kernel/kernel64 && cargo test --lib --target x86_64-unknown-linux-gnu devicetree
```

should end `test result: ok. 22 passed`. It takes a few seconds: one test reads eleven thousand damaged trees.

Automated: `make test64`
