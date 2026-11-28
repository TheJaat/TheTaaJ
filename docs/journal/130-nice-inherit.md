# A child takes its parent's nice

*Journal entry #130 · a step, MaK-WSL/sched's K190. Written on MaK-WSL/sched. Since entry 124 a program could make itself less important (`nice 10`), but whatever it started began at 0 again. Now a child starts at its parent's priority, the way it already takes its parent's memory limit (entry 096), and never above its own ceiling.*

## Why

`nice -n 10 make` is meant to put the whole build behind everything else: make, the compilers it runs, and theirs. With every child starting at 0, only the first program was behind; everything it started competed as equals. Entry 124 left this out on purpose (its design table: "inheriting from whoever loads you would give every program init's settings") and named the way to do it: the way the memory limit travels.

## How it works

Every program here is loaded by init: `posix_spawn` and the shell read the program's file into a memory object of their own, and ask init to start a program from it (SPAWN_IMAGE). So "the parent" is not the loader. It is whoever made the file's object, which entry 096 already uses for the memory limit.

- **MEMORY_CREATE** records the maker's priority in the new object, next to the account its pages are charged to.
- **SPAWN_IMAGE**, starting a program from an object, gives the new process that priority before its first thread exists, so the thread starts at it too.
- **Clamped to the child's own ceiling** (0 for everything but init). init puts the shell at -5 (entry 124), and the shell makes the objects of everything typed at it. Without the clamp, every program typed would start at -5, as important as the keyboard's driver. With it, a program at `nice 10` starts its children at 10, and the shell's start at 0.

A child can still move back up to its ceiling, 0, as its parent could have. Linux does not allow that without a privilege; here the ceiling is the privilege, and it is the same for parent and child. A stricter rule (a child's ceiling no higher than its parent's priority) would need its own item.

## How I checked it

- `schedbench` (all three boots): at nice 7 it starts a copy of itself that returns its priority as its exit code - "a child of a program at nice 7 runs at nice 7"; `schedbench64-test` requires the line.
- `boot64-test`: clib, which the shell (at -5) starts, checks first that its own priority is 0: the clamp. 156 checks pass.
- `make test64`.

## Testing note

```
make schedbench64-test
```

| Look for | You should see |
|---|---|
| `schedbench: inherit:` | `a child of a program at nice 7 runs at nice 7` |

Automated: `make test64`
