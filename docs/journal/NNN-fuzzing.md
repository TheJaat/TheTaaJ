# Fuzzing the parsers on the host: nine targets, LLVM's libFuzzer, and two bugs in the first minute

*Journal entry #NNN · K189, K104's second half. Written on TheJat-WSL/ci, by day. K104 (entry 123) fuzzed TheTaaJ's system calls from inside the machine. This is the other half: each parser of untrusted input fuzzed on the host, by a fuzzer that steers by which branches of the parser an input reached.*

## No crate from outside

`cargo fuzz` would need `cargo-fuzz` and its crate `libfuzzer-sys` from crates.io, which builds libFuzzer from source. The kernel's Cargo.toml says "Every line of the kernel is ours to read", and it has no crate from outside. Neither is needed:

- **libFuzzer is LLVM's own.** The project's LLVM 18.1.3 (`~/opt/llvm-18.1.3`) has `libclang_rt.fuzzer-x86_64.a`. It has the `main`, makes up the inputs and calls the target.
- **The coverage hooks are rustc's own, on the stable compiler**: `-C passes=sancov-module` with LLVM's options for the counters, the table of where they are, and comparisons. These are the flags `cargo fuzz` passes, and they need no nightly. K189 had said a nightly would be needed.

`fuzz/` is a crate of its own beside the others. It pins the kernel's Rust (1.100.0) and depends on the parsers' crates by path. Each program in `fuzz/src/bin/` is one target, written with a three-line macro (`fuzz::fuzz_target!`). `tools/fuzz.sh` builds them with those flags and links libFuzzer. It finds the library in `FUZZER_LIB`, then in the project's LLVM, then in the host's clang (`apt install libclang-rt-dev`). The release build keeps overflow checks and debug assertions, as host tests do: an overflow found here is one the kernel's build would have wrapped round silently.

## The targets

| Target | What it fuzzes | Edges reached (first runs) |
|---|---|---|
| `elf` | `kernel64::elf::parse_for`, both architectures | 107 (20 s) |
| `devicetree` | `Tree::new`, every node walked, `Machine::read` | 1647 (30 s) |
| `acpi_tables` | RSDP, RSDT/XSDT, MADT, FADT, from a made-up memory | 150 |
| `acpi_aml` | `acpi::sleep_type` (the power service's `\_S5`) | 59 (30 s) |
| `usb_descriptors` | a device's and a configuration's descriptors | 61 |
| `net_wire` | Ethernet, ARP, IPv4, ICMP, UDP, TCP, followed down a frame and each alone | 125 |
| `http` | the HTTP server's request reader | 175 |
| `fileserver` | the file protocol: a sequence of messages served by one store in memory, then paths | 740 |
| `taajfs` | a valid volume (files, a directory, a file of four blocks), edited by the input, then mounted and walked, read, written | 797 |

Runs were 60 s each unless the table says otherwise, on a quarter of the CPUs.

Two kinds of target need help:

- **Big valid inputs.** A TaajFS volume is 4-KiB blocks, and no fuzzer finds one from random bytes. So the `taajfs` target makes a valid volume and the input edits it, five bytes per edit (block, offset, byte).
- **Seeds.** An ELF file is refused below 64 bytes, and libFuzzer's first inputs are a few bytes long. Unseeded, `elf` reached 8 edges in 175 million runs. So when a target's corpus is empty, `fuzz.sh` seeds it from what the build already has:
  - ELF: the user programs under 64 KiB;
  - device tree: QEMU's own trees for its `virt` machine (`dumpdtb`);
  - network: frames from the TCP test's packet file (entry 156);
  - AML: QEMU's DSDT as `acpi/src/tests.rs` makes it;
  - HTTP: a written request.

  With seeds, `elf` reached 107 edges and `acpi_aml` 59.

What a target found interesting is kept in `build/fuzz/corpus/TARGET`, so a run goes on where the last one stopped.

## What it found

Two bugs, each within the first minute of its target:

- **`devicetree.rs:541`, `gic_interrupt`**: `number + 16` (or `+ 32`) overflows for an interrupt number near 2³². The kernel's build wraps it round, which means a wrong interrupt, silently. Filed as **K?-gic-number**.
- **`usb/src/descriptor.rs:135`, `configuration`**: a configuration that gives a total length under 6 has its bytes cut to that length, then `bytes[5]` is read. Any device plugged in can do that, and the `usb` program panics, again at every restart. The smallest input is `09 02 02 00 00 00 00 00 00`. Filed as **K?-usb-short-config**.

Both files are the `devices` lane's (MaK-WSL), so `ci` does not fix them. The items are in `docs/items/era-15.md`, with a line in `docs/lanes.md` for `devices` to take them into its table, as K186 went to `services`. When they are fixed, each input becomes a host test beside the parser's others, as K104 did.

ELF, ACPI tables and AML, the network formats, HTTP, the file server and TaajFS ran without a crash in these first minutes. That is not a proof: a longer run, at night, is the next step.

## `tools/fuzz.sh`

- `build`, `list`, `run [SECONDS] [TARGET...]`; `make fuzz` (`FUZZ_SECONDS`, default 60).
- `FUZZ_JOBS` processes per target (`-fork`), by default a quarter of the CPUs. By day, other sessions' timed boots share the machine.
- `-timeout=10`: an input that takes over 10 s is reported as a hang. libFuzzer's own limit, 20 minutes, kept one run going long past its time.
- A crash is kept in `build/fuzz/crashes/TARGET/`. libFuzzer keeps every crashing input, and one bug was 5,331 of them, so the run replays the smallest 50 and names each place a panic happened once, with its smallest input.
- Not part of `make test64`: it finds new inputs, it does not check old ones. A crash it finds becomes an ordinary host test.

## For the morning

- **Fuzzing at night:** the runner could give each target its minutes after the hunt (J95), with all CPUs.
- **More targets:** the TCP state machine (`net/src/tcp.rs`, segments into a connection), the DNS client, the other drivers' crates (`ahci`, `nvme`, `virtio`, `e1000`): what a device writes into memory is untrusted too.
- On TheJat-VM and MaK-WSL, libFuzzer is in the same LLVM 18.1.3, or in the host's clang with `libclang-rt-dev`.

## Testing note

| Do | You should see |
|---|---|
| `tools/fuzz.sh run 20 elf` | `fuzz: elf: 20 s, cov: ...` over 100, `crashes 0` |
| `tools/fuzz.sh run 30 usb_descriptors` | `crashes` many, and one place: `usb/src/descriptor.rs:135:45: index out of bounds` (until K?-usb-short-config is fixed) |
| `make fuzz FUZZ_SECONDS=10` | Each of the nine targets, its edges and corpus |

Automated: not part of `make test64`.
