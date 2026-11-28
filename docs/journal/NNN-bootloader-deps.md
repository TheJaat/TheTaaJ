# TheBootloader rebuilds what an edited include file touches

*Journal entry #NNN · a fix to TheBootloader's build, asked for in entry 148. Written on MaK-WSL/boot. The night tree's `k130-any-size` failed to link stage 2 - `undefined reference to 'BootHeader'` - because its `stage2.o` had been assembled for the branch before, and TheBootloader's Makefiles did not know that `stage2.asm` includes `datastructure.inc`, where K130 added `global BootHeader`. Now every object depends on the files it includes, as the assembler and the compiler list them.*

## Why

TheBootloader's rules said only "an object depends on its source": `stage2.o` on `stage2.asm`, `elf.elf` on `elf.c`. Stage 2 is mostly `%include`s (about twenty `.inc` files) and its C files include a dozen headers; an edit to any of those built nothing again. One tree building different commits one after another - the night tree, or anyone switching branches - links objects made from older include files. Entry 148 made the night runner throw TheBootloader's `build/` away when its commit changes, which keeps the night safe; this fixes the cause, for every tree.

## The change

In `src/boot/stage1/Makefile` and `src/boot/stage2/Makefile` (TheBootloader):

- `nasm -MD FILE.d -MP`: while assembling, nasm writes the files the source included to a `.d` file beside the object, in make's own syntax; `-MP` adds an empty rule for each, so a file removed later does not stop the build ("no rule to make target").
- `gcc -MMD -MP` for stage 2's C files: the same, for headers.
- `-include` of the `.d` files at the end of each Makefile (`-`: none exist in a fresh tree, and nothing is missing then, as everything is built).

**`revision.h` is left out** of the C files' lists. TheBootloader's `make` runs `increase-build` first every time, which rewrites `headers/revision.h`; `print.c` includes it, so `print.elf` and `stage2.bin` would be built again on every `make`, which they are not today. A `sed` takes its path out of `print.d` after each compile: the path only, not its line, because the line before ends in `\` and would otherwise join the next rule (my first try did exactly that: "print.d:1: *** multiple target patterns", bug 1).

The UEFI loader's Makefile is unchanged: it lists its four headers by hand, which is right for its one source file.

## Bugs

**1. Deleting a dependency's whole line broke the `.d` file.** The first version removed `revision.h` with `sed '/revision\.h/d'`. In `print.d` it was the last dependency, so the line before it ended in a `\` that now continued into the next rule's line: `print.d:1: *** multiple target patterns. Stop.` - but only on the second build, when the `.d` files are read; the first, fresh one passed, and my check's `grep` for what was rebuilt hid the failed exit code. Fixed: only the path is removed (`s| [^ ]*revision\.h||`), and its `-MP` rule; every check below prints the build's exit code.

## How I checked it

In `TheTaaJ-boot`, `make build-bootloader`, each build's exit code and what it assembled, compiled and linked:

| Done | Built again | Exit |
|---|---|---|
| A fresh build | everything; 9 `.d` files | 0 |
| Nothing changed | nothing | 0 |
| `datastructure.inc` touched | `stage2.asm`, the link | 0 |
| `bios_disk.h` touched | `ISO9660FS.c` (the one file that includes it), the link | 0 |
| `headers/boot/disk.inc` touched (stage 1's and stage 2's) | `stage1.asm`, `stage2.asm`, the link | 0 |
| `stage1/stage1_strings.inc` touched | nothing - no source includes it | 0 |

**The night's failure, replayed** in a scratch worktree of TheBootloader: built at `k23-boot-time`'s commit (`50691fc`), then switched to `k130-any-size`'s (`d898aab`) and built again without cleaning:

| Makefiles | Second build |
|---|---|
| Before (`d898aab`'s) | exit 2: ``undefined reference to `BootHeader'`` - the night's error |
| With this change | exit 0: assembled `stage2.asm` again (its `.inc` changed), compiled `elf.c` and `vfs.c` (changed by K130), linked |

Then the night run's whole `make test64` (below).

- `make test64` (night run): RESULT.

## What I learned

- **A build that passes from clean says nothing about the next one.** Every build from nothing passed; the bug lived only in a tree reused across commits. The check that matters is the second build, after a change.
- **A check that greps the output must also look at the exit code** (bug 1).

## Testing note

```
make build-bootloader
touch bootloader/headers/boot/stage2/asm_includes/datastructure.inc
make build-bootloader
```

| Look for | You should see |
|---|---|
| The second build | `Assembling stage2.asm`, then `Linking .../stage2.bin` - and no other file assembled or compiled |
| `make build-bootloader` once more, nothing touched | nothing assembled, compiled or linked |

Automated: `make test64` builds TheBootloader; the replay above is by hand.
