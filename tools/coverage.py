#!/usr/bin/env python3
"""coverage.py [--raise] [CRATE...] - how much of each crate's code its
host tests run (J96, journal entry NNN).

  make coverage           the table, and a failure if a crate fell
                          below its floor
  make coverage-raise     the same, then the floor raised to today's
                          numbers where they are higher

Every crate `make kernel64-test` tests (the kernel library for the host,
as that does), built with rustc's own instrumentation
(`-C instrument-coverage`) by the Rust the kernel is pinned to, its
`cargo test` run, and the counts read with that Rust's llvm-cov
(rustup's `llvm-tools`, installed the first time). Only the crate's own
`src/` is counted - not the crates it uses, which are counted as
themselves - and not its tests: test files (`tests.rs`, `tests/`), and
every `#[cfg(test)]` item in the others (most crates keep their tests in
a `mod tests` at the end of the file; counted, the tests' own lines -
all run - would make up a fifth of the "covered").

The floor (tools/coverage-floor): a crate's lines covered, in whole
percent, rounded down. A crate under its floor fails: a change that
adds code without tests, or removes tests, shows. The floor only rises
(--raise); lowering it is done by hand, in a commit that says why.

Its own build directory (build/coverage), so the normal builds are not
rebuilt after it. Not part of make test64: a second build of every
crate, about 3 minutes.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FLOOR = os.path.join(ROOT, "tools/coverage-floor")
OUT = os.path.join(ROOT, "build/coverage")
CRATES = ["kernel/kernel64", "kernel/abi", "input", "console", "heap", "fileserver", "registry", "taajfs",
          "virtio", "ahci", "nvme", "acpi", "e1000", "usb", "net", "gfx", "gui"]
HOST = "x86_64-unknown-linux-gnu"
IGNORE = r"(/\.cargo/|/rustc/|/tests\.rs$|/tests/|/build/coverage/)"


def toolchain():
    """The Rust the kernel is pinned to (rust-toolchain.toml)."""
    for line in open(os.path.join(ROOT, "kernel/kernel64/rust-toolchain.toml")):
        if line.strip().startswith("channel"):
            return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit("coverage: no channel in kernel/kernel64/rust-toolchain.toml")


def tools(rust):
    sysroot = subprocess.run(["rustup", "run", rust, "rustc", "--print", "sysroot"],
                             capture_output=True, text=True, check=True).stdout.strip()
    bin_dir = os.path.join(sysroot, "lib/rustlib", HOST, "bin")
    if not os.path.exists(os.path.join(bin_dir, "llvm-cov")):
        subprocess.run(["rustup", "component", "add", "llvm-tools", "--toolchain", rust],
                       capture_output=True, check=True)
    return os.path.join(bin_dir, "llvm-profdata"), os.path.join(bin_dir, "llvm-cov")


def floors():
    found = {}
    if os.path.exists(FLOOR):
        for line in open(FLOOR):
            line = line.split("#", 1)[0].split()
            if len(line) == 2:
                found[line[0]] = int(line[1])
    return found


def measure(crate, rust, profdata, cov):
    """(lines covered %, functions covered %, lines), or raises."""
    where = os.path.join(ROOT, crate)
    raw = os.path.join(OUT, "raw", crate.replace("/", "-"))
    shutil.rmtree(raw, ignore_errors=True)
    os.makedirs(raw)
    env = dict(os.environ,
               RUSTFLAGS="-C instrument-coverage",
               CARGO_TARGET_DIR=os.path.join(OUT, "target"),
               LLVM_PROFILE_FILE=os.path.join(raw, "%p-%m.profraw"))
    cargo = ["rustup", "run", rust, "cargo", "test", "--tests", "--target", HOST]
    done = subprocess.run(cargo + ["-q"], cwd=where, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        tail = (done.stdout + done.stderr).strip().splitlines()[-6:]
        raise RuntimeError("its tests failed:\n    " + "\n    ".join(tail))
    listed = subprocess.run(cargo + ["--no-run", "--message-format=json"], cwd=where, env=env,
                            capture_output=True, text=True, check=True).stdout
    programs = [m["executable"] for m in map(json.loads, listed.splitlines())
                if m.get("reason") == "compiler-artifact" and m.get("executable")
                and m.get("profile", {}).get("test")]
    profiles = [os.path.join(raw, f) for f in os.listdir(raw)]
    if not programs or not profiles:
        raise RuntimeError("no test program ran")
    merged = os.path.join(raw, "merged.profdata")
    subprocess.run([profdata, "merge", "-sparse", "-o", merged] + profiles, check=True)
    objects = [programs[0]] + [x for p in programs[1:] for x in ("-object", p)]
    report = subprocess.run([cov, "export", "-format=lcov", "-instr-profile", merged,
                             f"-ignore-filename-regex={IGNORE}"] + objects + [os.path.join(where, "src")],
                            capture_output=True, text=True, check=True).stdout
    return count(report)


def test_lines(path):
    """The lines of a file's `#[cfg(test)]` items, and the files of its
    `#[cfg(test)] mod NAME;`."""
    try:
        text = open(path).read().splitlines()
    except OSError:
        return set(), []
    lines, files, i = set(), [], 0
    while i < len(text):
        if not re.match(r"\s*#\[cfg\(test\)\]", text[i]):
            i += 1
            continue
        start = i
        i += 1
        while i < len(text) and re.match(r"\s*#\[", text[i]):
            i += 1  # more attributes on the same item
        if i >= len(text):
            break
        item = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", text[i])
        if item:
            base = os.path.dirname(path)
            files += [os.path.join(base, item.group(1) + ".rs"), os.path.join(base, item.group(1), "mod.rs")]
        depth, opened = 0, False
        while i < len(text):
            # Braces in strings and characters don't count.
            code = re.sub(r'"(\\.|[^"\\])*"|\'(\\.|[^\'\\])\'|//.*', "", text[i])
            depth += code.count("{") - code.count("}")
            opened = opened or "{" in code
            if (opened and depth <= 0) or (not opened and code.rstrip().endswith(";")):
                break
            i += 1
        lines.update(range(start + 1, i + 2))  # 1-based, both ends
        i += 1
    return lines, files


def count(lcov):
    """(lines covered %, functions covered %, lines) from llvm-cov's lcov,
    test items left out."""
    lines = covered = functions = functions_run = 0
    skipped_files = set()
    records = lcov.split("end_of_record")
    for record in records:
        name = re.search(r"^SF:(.*)$", record, re.M)
        if name:
            skipped_files.update(test_lines(name.group(1))[1])
    for record in records:
        name = re.search(r"^SF:(.*)$", record, re.M)
        if not name or name.group(1) in skipped_files:
            continue
        tests, _ = test_lines(name.group(1))
        for line, hits in re.findall(r"^DA:(\d+),(\d+)", record, re.M):
            if int(line) not in tests:
                lines += 1
                covered += int(hits) > 0
        starts = {fn: int(line) for line, fn in re.findall(r"^FN:(\d+),(.*)$", record, re.M)}
        for hits, fn in re.findall(r"^FNDA:(\d+),(.*)$", record, re.M):
            if starts.get(fn) not in tests:
                functions += 1
                functions_run += int(hits) > 0
    if not lines:
        raise RuntimeError("no lines counted")
    return 100.0 * covered / lines, 100.0 * functions_run / max(functions, 1), lines


def main():
    args = sys.argv[1:]
    raising = "--raise" in args
    crates = [a for a in args if a != "--raise"] or CRATES
    rust = toolchain()
    profdata, cov = tools(rust)
    floor = floors()
    measured, failed = {}, False
    print(f"{'crate':<16} {'lines':>7} {'funcs':>7} {'of lines':>8}  floor  seconds")
    for crate in crates:
        start = time.time()
        try:
            lines, functions, count = measure(crate, rust, profdata, cov)
        except (RuntimeError, subprocess.CalledProcessError) as why:
            print(f"{crate:<16} FAILED: {why}")
            failed = True
            continue
        measured[crate] = lines
        least = floor.get(crate)
        mark = ""
        if least is not None and int(lines) < least:
            mark = "  BELOW ITS FLOOR"
            failed = True
        print(f"{crate:<16} {lines:6.1f}% {functions:6.1f}% {count:8}  {least if least is not None else '-':>5}  "
              f"{int(time.time() - start):7}{mark}", flush=True)
    if raising:
        for crate, lines in measured.items():
            floor[crate] = max(floor.get(crate, 0), int(lines))
        with open(FLOOR, "w") as out:
            out.write("# Each crate's host tests cover at least this many percent of its lines\n"
                      "# (tools/coverage.py; J96, journal entry NNN). Raised by\n"
                      "# `make coverage-raise`; lowered only by hand, saying why.\n")
            for crate in CRATES + sorted(set(floor) - set(CRATES)):
                if crate in floor:
                    out.write(f"{crate} {floor[crate]}\n")
        print(f"coverage: floor written to {os.path.relpath(FLOOR, ROOT)}")
    if failed:
        print("coverage: FAILED - a crate's tests failed, or it fell below its floor")
        sys.exit(1)
    print("coverage: every crate at or above its floor")


main()
