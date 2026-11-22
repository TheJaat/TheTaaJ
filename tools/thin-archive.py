#!/usr/bin/env python3
"""thin-archive.py NM AR ARCHIVE - keeps, of a C library archive, only
the members a C program can reach, and writes the archive back.

libc.a is the Rust half of the C library (one object, after link-time
optimisation), the C half (an object per file), and Rust's support
library, compiler_builtins: four hundred objects of helpers and maths
functions, most of which no C program here ever calls. Copied whole,
the archive is over 5 MB - more than a file on TaajFS may hold (4 MB),
and a lot for a compiler on TheTaaJ to read every time it links.

What is kept: every member that is not compiler_builtins, and then,
again and again until nothing changes, every compiler_builtins member
that defines a symbol something already kept leaves undefined - plus
the five that a compiler may call without being asked (memcpy, memmove,
memset, memcmp and bcmp, for copying structures and the like). Members
whose symbols have all been made local (the soft-float helpers, see
LIBC_RUST_PRIVATE in the Makefile) define nothing anyone can ask for,
and go.

The order of the members is kept.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile

# What a compiler may call by itself, with no call in the source.
ALWAYS = {"memcpy", "memmove", "memset", "memcmp", "bcmp"}

# compiler_builtins' members: its Rust objects, and the C and assembly
# it has besides (the AArch64 atomics, a few of LLVM's own helpers),
# which come named by a hash and the file: 883bfc136aede933-lse_cas1_acq.o.
SUPPORT = re.compile(r"^(compiler_builtins|[0-9a-f]{16}-)")


def symbols(nm, path, *flags):
    out = subprocess.run([nm, *flags, path], capture_output=True, text=True, check=True).stdout
    return {line.split()[-1] for line in out.splitlines() if line.strip()}


def main():
    nm, ar, archive = sys.argv[1:4]
    with tempfile.TemporaryDirectory() as work:
        names = subprocess.run([ar, "t", archive], capture_output=True, text=True, check=True).stdout.split()
        if len(set(names)) != len(names):
            sys.exit(f"thin-archive: {archive} has two members of one name: they cannot be told apart")
        subprocess.run([ar, "x", os.path.abspath(archive)], cwd=work, check=True)
        defined = {m: symbols(nm, os.path.join(work, m), "--defined-only", "-g") for m in names}
        undefined = {m: symbols(nm, os.path.join(work, m), "-u") for m in names}
        keep = {m for m in names if not SUPPORT.match(m)}
        need = set(ALWAYS).union(*(undefined[m] for m in keep))
        # What is kept already defines some of that: a member is taken
        # only for a symbol nothing kept has, so that no symbol is
        # defined twice (tools/build-tcc.sh merges the Rust half into one
        # object, where two definitions would be an error).
        have = set().union(*(defined[m] for m in keep))
        changed = True
        while changed:
            changed = False
            for m in names:
                if m not in keep and defined[m] & (need - have):
                    keep.add(m)
                    need |= undefined[m]
                    have |= defined[m]
                    changed = True
        kept = [m for m in names if m in keep]
        before = os.path.getsize(archive)
        thin = os.path.join(work, "thin.a")
        subprocess.run([ar, "rcs", thin, *kept], cwd=work, check=True)
        shutil.copyfile(thin, archive)
        print(f"thin-archive: {archive}: {len(kept)} of {len(names)} members, {before} -> {os.path.getsize(archive)} bytes")


if __name__ == "__main__":
    main()
