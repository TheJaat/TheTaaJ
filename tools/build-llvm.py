#!/usr/bin/env python3
"""build-llvm.py - LLVM (ports/llvm/llvm), cross-compiled for TheTaaJ.

    usage: build-llvm.py ARCH OUT CXXFLAGS CFLAGS TOOL...

    ARCH      x86_64 or aarch64
    OUT       the directory to build in (made if need be)
    CXXFLAGS  clang's flags for a TheTaaJ C++ program, include paths and
              all (TAAJ_LLVM_CXXFLAGS in the Makefile)
    CFLAGS    the same for C (LLVM has a few C files: BLAKE3)
    TOOL      the programs to build: llvm-tblgen, ...

LLVM's own build is CMake, which TheTaaJ's host does not have and which
does not know TheTaaJ. What CMake would do for this configuration is
small enough to do here, and its CMakeLists.txt files - kept in
ports/llvm - still say what to do:

  1. Configure: the headers CMake writes from templates
     (include/llvm/Config/*.cmake and *.in), with the values in
     ports/llvm/taaj/llvm-config.txt.
  2. TableGen: every `tablegen(LLVM out.inc -gen-...)` of the
     directories a library needs, run with the host's llvm-tblgen of
     the same release (LLVM_TBLGEN), into OUT/gen.
  3. Compile each library's sources - the .cpp and .c files listed in
     its add_llvm_component_library(...) - and archive them.
  4. Link each tool with the libraries it needs.

What is built again is decided as make would: an object is compiled when
its source or anything it included (clang's -MD list) is newer than it,
and a generated file when its .td files are newer. The compiles run side
by side, one per CPU of the host.
"""

import concurrent.futures
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LLVM = os.path.join(ROOT, "ports", "llvm", "llvm")
TAAJ = os.path.join(ROOT, "ports", "llvm", "taaj")
TBLGEN = os.environ.get("LLVM_TBLGEN", "llvm-tblgen-18")
AR = os.environ.get("LLVM_AR", "llvm-ar-18")
JOBS = os.cpu_count() or 4

# The libraries: where their CMakeLists.txt is, under which name it
# declares them, and the directories whose tablegen() lines they need
# first. Only what the tools below are made of.
LIBRARIES = {
    "LLVMDemangle": ("lib/Demangle", []),
    "LLVMSupport": ("lib/Support", []),
    "LLVMTableGen": ("lib/TableGen", []),
    "LLVMCodeGenTypes": ("lib/CodeGen", ["include/llvm/CodeGen"]),
    "LLVMTableGenGlobalISel": ("utils/TableGen/GlobalISel", ["include/llvm/CodeGen"]),
    "LLVMTableGenCommon": ("utils/TableGen", ["include/llvm/CodeGen", "include/llvm/IR"]),
}

# Libraries CMake makes OBJECT libraries: their objects are linked
# into the tool one by one, not taken from an archive as needed. Each
# TableGen backend registers itself ("-gen-vt", ...) from a constructor
# in its own file, and nothing refers to that file by name - from an
# archive, the linker would leave every backend out.
OBJECT_LIBRARIES = {"LLVMTableGenCommon"}

# The tools: their CMakeLists.txt, their name there, and the libraries
# they are linked with, the ones that need others first.
TOOLS = {
    "llvm-tblgen": ("utils/TableGen", ["LLVMTableGenGlobalISel", "LLVMTableGenCommon", "LLVMTableGen",
                                       "LLVMCodeGenTypes", "LLVMSupport", "LLVMDemangle"]),
}

# The parts of BLAKE3 (a hash, in C) without assembly or SIMD: the
# portable code only.
BLAKE3 = ["BLAKE3/blake3.c", "BLAKE3/blake3_dispatch.c", "BLAKE3/blake3_portable.c"]
BLAKE3_FLAGS = ["-DBLAKE3_NO_AVX512", "-DBLAKE3_NO_AVX2", "-DBLAKE3_NO_SSE41", "-DBLAKE3_NO_SSE2",
                "-DBLAKE3_USE_NEON=0"]


def fail(message):
    print("build-llvm: " + message, file=sys.stderr)
    sys.exit(1)


def cmake_calls(path):
    """The calls in a CMakeLists.txt, as (name, [arguments]), comments
    and quotes taken off. Enough of CMake's syntax for LLVM's lists."""
    with open(path) as f:
        text = f.read()
    text = re.sub(r"#[^\n]*", "", text)
    calls = []
    for match in re.finditer(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(", text):
        depth, i = 1, match.end()
        while depth and i < len(text):
            depth += {"(": 1, ")": -1}.get(text[i], 0)
            i += 1
        body = text[match.end():i - 1]
        arguments = [a.strip('"') for a in body.split()]
        calls.append((match.group(1), arguments))
    return calls


def sources_of(directory, name):
    """The .cpp and .c files listed in the call that declares `name`."""
    for call, arguments in cmake_calls(os.path.join(LLVM, directory, "CMakeLists.txt")):
        if call in ("add_llvm_component_library", "add_llvm_library", "add_tablegen", "add_llvm_tool") \
                and arguments and arguments[0] == name:
            files = [a for a in arguments[1:] if re.search(r"\.(cpp|c)$", a) and "$" not in a]
            return [os.path.join(directory, f) for f in files]
    fail(f"no {name} in {directory}/CMakeLists.txt")


def tablegen_jobs(directory):
    """The tablegen(LLVM out.inc flags...) lines of a directory, as
    (td file, output, flags)."""
    jobs, definitions = [], None
    for call, arguments in cmake_calls(os.path.join(LLVM, directory, "CMakeLists.txt")):
        if call == "set" and arguments and arguments[0] == "LLVM_TARGET_DEFINITIONS":
            definitions = arguments[1].replace("${PROJECT_SOURCE_DIR}", LLVM)
            if not os.path.isabs(definitions):
                definitions = os.path.join(LLVM, directory, definitions)
        elif call == "tablegen" and arguments and arguments[0] == "LLVM":
            jobs.append((definitions, arguments[1], arguments[2:]))
    return jobs


def configure(template, output, values):
    """CMake's configure_file: #cmakedefine and #cmakedefine01 lines,
    ${NAME} and @NAME@."""
    def value(name):
        return values.get(name, "")

    lines = []
    with open(template) as f:
        for line in f:
            m = re.match(r"#cmakedefine01\s+(\w+)", line)
            if m:
                on = value(m.group(1)) not in ("", "0", "OFF", "FALSE")
                lines.append(f"#define {m.group(1)} {1 if on else 0}\n")
                continue
            m = re.match(r"#cmakedefine\s+(\w+)(.*)", line)
            if m:
                if value(m.group(1)) in ("", "0", "OFF", "FALSE"):
                    lines.append(f"/* #undef {m.group(1)} */\n")
                    continue
                line = f"#define {m.group(1)}{m.group(2)}\n"
            line = re.sub(r"\$\{(\w+)\}", lambda m: value(m.group(1)), line)
            line = re.sub(r"@(\w+)@", lambda m: value(m.group(1)), line)
            lines.append(line)
    write_if_changed(output, "".join(lines))


def write_if_changed(path, text):
    """Writes only what differs, so that what includes it is not
    compiled again for nothing."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        with open(path) as f:
            if f.read() == text:
                return
    with open(path, "w") as f:
        f.write(text)


def read_values():
    values = {}
    with open(os.path.join(TAAJ, "llvm-config.txt")) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                name, _, value = line.partition("=")
                values[name.strip()] = value.strip()
    return values


def newer(target, sources):
    """Is any of `sources` newer than `target` (or `target` missing)?"""
    if not os.path.exists(target):
        return True
    when = os.path.getmtime(target)
    return any(not os.path.exists(s) or os.path.getmtime(s) > when for s in sources)


def dependencies(depfile):
    """The files a compile read, from clang's -MD output."""
    try:
        with open(depfile) as f:
            text = f.read().replace("\\\n", " ")
    except FileNotFoundError:
        return None
    return text.split(":", 1)[1].split() if ":" in text else None


def run(command):
    result = subprocess.run(command, capture_output=True, text=True)
    return result.returncode, result.stdout + result.stderr


def main():
    if len(sys.argv) < 6:
        fail("usage: build-llvm.py ARCH OUT CXXFLAGS CFLAGS TOOL...")
    arch, out, cxxflags, cflags = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3].split(), sys.argv[4].split()
    tools = sys.argv[5:]
    if arch not in ("x86_64", "aarch64"):
        fail("no such CPU: " + arch)
    for tool in tools:
        if tool not in TOOLS:
            fail("no such tool: " + tool)
    keep_going = os.environ.get("KEEP_GOING") == "1"
    gen = os.path.join(out, "gen")

    # 1. The configured headers.
    values = read_values()
    values["LLVM_HOST_TRIPLE"] = f"{arch}-unknown-taaj"
    values["LLVM_DEFAULT_TARGET_TRIPLE"] = f"{arch}-unknown-taaj"
    values["LLVM_NATIVE_ARCH"] = "X86" if arch == "x86_64" else "AArch64"
    config = os.path.join(LLVM, "include", "llvm", "Config")
    for name in ("config.h", "llvm-config.h", "abi-breaking.h"):
        configure(os.path.join(config, name + ".cmake"), os.path.join(gen, "include/llvm/Config", name), values)
    targets = ["X86", "AArch64"]
    for kind, macro, enum in (("Targets", "TARGET", "TARGETS"), ("AsmPrinters", "ASM_PRINTER", "ASM_PRINTERS"),
                              ("AsmParsers", "ASM_PARSER", "ASM_PARSERS"),
                              ("Disassemblers", "DISASSEMBLER", "DISASSEMBLERS"),
                              ("TargetMCAs", "TARGETMCA", "TARGETMCAS"), ("TargetExegesis", "EXEGESIS", "EXEGESIS")):
        values[f"LLVM_ENUM_{enum}"] = "".join(f"LLVM_{macro}({t})\n" for t in targets)
        configure(os.path.join(config, kind + ".def.in"), os.path.join(gen, "include/llvm/Config", kind + ".def"),
                  values)
    write_if_changed(os.path.join(gen, "include/llvm/Support/VCSRevision.h"), "#undef LLVM_REVISION\n")

    # 2. TableGen, for the libraries the tools need.
    needed = []
    for tool in tools:
        for library in TOOLS[tool][1]:
            if library not in needed:
                needed.append(library)
    directories = []
    for library in needed:
        for d in LIBRARIES[library][1]:
            if d not in directories:
                directories.append(d)
    for directory in directories:
        for td, output, flags in tablegen_jobs(directory):
            target = os.path.join(gen, directory, output)
            depfile = target + ".d"
            deps = dependencies(depfile) or [td]
            if not newer(target, deps):
                continue
            print(f"  TBLGEN  {directory}/{output}")
            os.makedirs(os.path.dirname(target), exist_ok=True)
            command = [TBLGEN, *flags, "-I", os.path.join(LLVM, directory), "-I", os.path.dirname(td),
                       "-I", os.path.join(LLVM, "include"), td, "-o", target, "-d", depfile]
            code, text = run(command)
            if code:
                fail(f"llvm-tblgen failed on {output}:\n{text}")

    # 3. The libraries.
    includes = ["-I", os.path.join(gen, "include"), "-I", os.path.join(LLVM, "include")]
    for directory in directories:
        includes += ["-I", os.path.join(gen, directory)]
    common = ["-D__STDC_CONSTANT_MACROS", "-D__STDC_FORMAT_MACROS", "-D__STDC_LIMIT_MACROS", "-DNDEBUG",
              "-ffunction-sections", "-fdata-sections"]
    work = []
    archives = {}
    for library in needed:
        directory = LIBRARIES[library][0]
        sources = sources_of(directory, library)
        if library == "LLVMSupport":
            sources += [os.path.join(directory, f) for f in BLAKE3]
        objects = []
        for source in sources:
            obj = os.path.join(out, "obj", re.sub(r"\.(cpp|c)$", ".o", source))
            objects.append(obj)
            depfile = obj[:-2] + ".d"
            deps = dependencies(depfile)
            if deps is not None and not newer(obj, deps):
                continue
            path = os.path.join(LLVM, source)
            if source.endswith(".c"):
                command = ["clang", *cflags, *common, *BLAKE3_FLAGS, *includes]
            else:
                command = ["clang++", *cxxflags, *common, *includes, "-I", os.path.join(LLVM, directory)]
                if library.startswith("LLVMTableGen") and directory.startswith("utils"):
                    command += ["-I", os.path.join(LLVM, "utils/TableGen")]
            command += ["-MD", "-MF", depfile, "-c", path, "-o", obj]
            work.append((source, obj, command))
        archives[library] = (os.path.join(out, "lib", library + ".a"), objects)

    # The tools' own sources.
    tool_objects = {}
    for tool in tools:
        directory, _ = TOOLS[tool]
        objects = []
        for source in sources_of(directory, tool):
            obj = os.path.join(out, "obj", re.sub(r"\.(cpp|c)$", ".o", source))
            objects.append(obj)
            depfile = obj[:-2] + ".d"
            deps = dependencies(depfile)
            if deps is not None and not newer(obj, deps):
                continue
            command = ["clang++", *cxxflags, *common, *includes, "-I", os.path.join(LLVM, directory),
                       "-MD", "-MF", depfile, "-c", os.path.join(LLVM, source), "-o", obj]
            work.append((source, obj, command))
        tool_objects[tool] = objects

    failures = []
    if work:
        print(f"  CXX     {len(work)} files of LLVM ({arch}), {JOBS} at a time")
        with concurrent.futures.ThreadPoolExecutor(JOBS) as pool:
            def compile_one(job):
                source, obj, command = job
                os.makedirs(os.path.dirname(obj), exist_ok=True)
                return source, run(command)
            for source, (code, text) in pool.map(compile_one, work):
                if code:
                    failures.append(source)
                    print(f"build-llvm: {source} did not compile:\n{text}", file=sys.stderr)
                    if not keep_going:
                        pool.shutdown(cancel_futures=True)
                        break
    if failures:
        fail(f"{len(failures)} files did not compile")

    for library, (archive, objects) in archives.items():
        if newer(archive, objects):
            os.makedirs(os.path.dirname(archive), exist_ok=True)
            if os.path.exists(archive):
                os.remove(archive)
            code, text = run([AR, "rcs", archive, *objects])
            if code:
                fail(f"cannot archive {library}:\n{text}")

    # 4. The tools. TAAJ_LLVM_LINK says how: the linker and its flags,
    # with {objects} and {output} in their places.
    link = os.environ.get("TAAJ_LLVM_LINK")
    if not link:
        fail("TAAJ_LLVM_LINK is not set")
    for tool in tools:
        output = os.path.join(out, "bin", tool)
        objects = list(tool_objects[tool])
        for l in TOOLS[tool][1]:
            if l in OBJECT_LIBRARIES:
                objects += archives[l][1]
        libraries = [archives[l][0] for l in TOOLS[tool][1] if l not in OBJECT_LIBRARIES]
        # The files the link command names (libc.a, libc++.a, the
        # linker script) count too: a new C library means a new tool.
        named = [w for w in link.split() if os.path.isfile(w)]
        if not newer(output, objects + libraries + named + [os.path.join(ROOT, "user", "cxx.ld")]):
            continue
        os.makedirs(os.path.dirname(output), exist_ok=True)
        print(f"  LD      {tool} ({arch})")
        command = []
        for word in link.split():
            if word == "{objects}":
                command += objects + ["--start-group"] + libraries + ["--end-group"]
            elif word == "{output}":
                command.append(output)
            else:
                command.append(word)
        code, text = run(command)
        if code:
            fail(f"cannot link {tool}:\n{text}")


if __name__ == "__main__":
    main()
