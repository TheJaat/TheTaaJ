# The night tree builds TheBootloader from nothing when its commit changes

*Journal entry #NNN · a fix to the night runner. Written on TheJat-WSL/ci. The second night's `k130-any-size` (MaK-WSL/boot) failed after 41 seconds, in `test64-shared`: TheBootloader's `stage2.bin` did not link, `undefined reference to 'BootHeader'`. MaK-WSL's own checks had passed, and a fresh build of the same commit passes. The night tree's build was stale.*

## What happened

The night tree builds one branch after another in the same `build/` directories. The boot stack's branches each point at their own TheBootloader commit, in a line: `k23-boot-time` at `50691fc4`, `k130-any-size` at `d898aabc`. `k130-any-size` adds `global BootHeader` to `headers/boot/stage2/asm_includes/datastructure.inc`, which `stage2`'s assembly includes, and `elf.c` now reads `BootHeader`. TheBootloader's Makefile does not list the `.inc` files an assembly file includes: `stage2.o`, assembled for `k23-boot-time` without that line, was not assembled again, and the link found no `BootHeader`.

Checked: in a fresh tree, `k130-any-size` with TheBootloader at `d898aabc`, `make build-bootloader` - passes.

## The fix

`prepare()` keeps the TheBootloader commit it built last (`build/.bootloader-commit`); when the branch's is another, it removes TheBootloader's `build/` first. TheBootloader builds in seconds; the rest of the tree stays incremental.

The cause is TheBootloader's Makefile, which is MaK-WSL's (rule 7): its `.o` rules could depend on the `.inc` files they include (`nasm -M` writes them). Told to MaK-WSL; this keeps the night safe either way.

## Testing note

| Do | You should see |
|---|---|
| A night queue with two branches pointing at different TheBootloader commits, the second changing an `.inc` | The second builds TheBootloader from nothing and passes |

Automated: not part of `make test64`.
