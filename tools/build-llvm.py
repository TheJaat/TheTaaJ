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
import struct
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LLVM = os.path.join(ROOT, "ports", "llvm", "llvm")
TAAJ = os.path.join(ROOT, "ports", "llvm", "taaj")
TBLGEN = os.environ.get("LLVM_TBLGEN", "llvm-tblgen-18")
AR = os.environ.get("LLVM_AR", "llvm-ar-18")
JOBS = os.cpu_count() or 4

# The libraries are found by reading LLVM's CMakeLists.txt files (see
# Components below): what each is made of, which others it needs
# (LINK_COMPONENTS) and which TableGen output it includes (DEPENDS).
# A tool's own list is its LLVM_LINK_COMPONENTS.
#
# llvm-tblgen is the one tool CMake builds differently, by hand-written
# lines in utils/TableGen/CMakeLists.txt rather than components: its
# backends are an OBJECT library (below), and its GlobalISel part is
# added with target_link_libraries.
TOOL_DIRECTORIES = {
    "llvm-tblgen": "utils/TableGen",
    "llvm-mc": "tools/llvm-mc",
    "llc": "tools/llc",
}
EXTRA_LIBRARIES = {"llvm-tblgen": ["TableGenGlobalISel", "TableGenCommon"]}

# Libraries CMake makes OBJECT libraries: their objects are linked
# into the tool one by one, not taken from an archive as needed. Each
# TableGen backend registers itself ("-gen-vt", ...) from a constructor
# in its own file, and nothing refers to that file by name - from an
# archive, the linker would leave every backend out.
OBJECT_LIBRARIES = {"TableGenCommon"}

# The directories whose CMakeLists.txt declare libraries.
SCAN = ["lib", "utils/TableGen"]

# The targets built, and the parts of each that "AllTargets..." means.
TARGETS = ["X86", "AArch64"]
ALL_TARGETS = {
    "alltargetsasmparsers": "AsmParser",
    "alltargetscodegens": "CodeGen",
    "alltargetsdescs": "Desc",
    "alltargetsdisassemblers": "Disassembler",
    "alltargetsinfos": "Info",
    "alltargetsmcas": "TargetMCA",
}

# The keywords of add_llvm_component_library and its relations, which
# end the list of sources and begin another list.
KEYWORDS = {"ADDITIONAL_HEADER_DIRS", "ADDITIONAL_HEADERS", "LINK_LIBS", "DEPENDS", "LINK_COMPONENTS",
            "ADD_TO_COMPONENT", "COMPONENT_NAME", "DESTINATION", "EXPORT"}
# Of those, the ones followed by one word only, after which the list
# of sources goes on (add_tablegen's DESTINATION dir EXPORT LLVM a.cpp).
SINGLE = {"COMPONENT_NAME", "DESTINATION", "EXPORT"}
# And the ones that stand alone, saying something about the library
# without a list after them.
FLAGS = {"PARTIAL_SOURCES_INTENDED", "BUILDTREE_ONLY", "STATIC", "SHARED", "MODULE", "OBJECT", "EXCLUDE_FROM_ALL",
         "DISABLE_LLVM_LINK_LLVM_DYLIB", "SUPPORT_PLUGINS", "EXPORT_SYMBOLS", "NO_INSTALL_RPATH", "ENABLE_PLUGINS"}

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
        arguments = [a.replace('"', '') for a in body.split()]
        calls.append((match.group(1), arguments))
    return calls


def expanded(calls):
    """The calls with ${NAME} replaced by what set(NAME ...) said, for
    the lists LLVM keeps in variables (set(sources ...), and a tool's
    LLVM_LINK_COMPONENTS). A name never set is left out."""
    variables, result = {}, []
    for call, arguments in calls:
        words = []
        for a in arguments:
            m = re.fullmatch(r"\$\{(\w+)\}", a)
            words += variables.get(m.group(1), []) if m else [a]
        if call == "set" and words:
            variables[words[0]] = words[1:]
        result.append((call, words, dict(variables)))
    return result


def sections(arguments):
    """add_llvm_component_library's arguments by keyword: the files
    before any keyword under "", then each keyword's words."""
    found, current, single = {"": []}, "", False
    for a in arguments:
        if a in FLAGS:
            continue
        if a in KEYWORDS:
            current, single = a, a in SINGLE
            found.setdefault(a, [])
        else:
            found.setdefault(current, []).append(a)
            if single:
                current, single = "", False
    return found


def files(directory, words):
    return [os.path.join(directory, w) for w in words if re.search(r"\.(cpp|c)$", w) and "$" not in w]


class Component:
    def __init__(self, name, directory, sources, links, depends, group):
        self.name, self.directory, self.sources = name, directory, sources
        self.links, self.depends, self.group = links, depends, group


class Components:
    """Every library under SCAN, by component name (lower case: CMake's
    names for components are not case-sensitive), and every TableGen
    target (intrinsics_gen, X86CommonTableGen, ...) by the directory
    whose tablegen() lines it runs."""

    def __init__(self):
        self.by_name, self.tablegen = {}, {}
        for top in SCAN:
            for here, _, names in os.walk(os.path.join(LLVM, top)):
                if "CMakeLists.txt" in names:
                    self.read(os.path.relpath(here, LLVM))
        for here, _, names in os.walk(os.path.join(LLVM, "include")):
            if "CMakeLists.txt" in names:
                self.read(os.path.relpath(here, LLVM))

    def read(self, directory):
        calls = expanded(cmake_calls(os.path.join(LLVM, directory, "CMakeLists.txt")))
        for call, words, _ in calls:
            if call == "add_public_tablegen_target" and words:
                self.tablegen[words[0]] = directory
            if call not in ("add_llvm_component_library", "add_llvm_library", "add_llvm_target") or not words:
                continue
            name = words[0]
            if call == "add_llvm_target":
                name = "LLVM" + name
            parts = sections(words[1:])
            component = (parts.get("COMPONENT_NAME") or [name[4:] if name.startswith("LLVM") else name])[0]
            group = (parts.get("ADD_TO_COMPONENT") or [None])[0]
            library = Component(component, directory, files(directory, parts[""]),
                                parts.get("LINK_COMPONENTS", []), parts.get("DEPENDS", []), group)
            # Known by its component name (Scalar) and its library's
            # name without "LLVM" (ScalarOpts): tools use both.
            self.by_name[component.lower()] = library
            self.by_name[(name[4:] if name.startswith("LLVM") else name).lower()] = library

    def expand(self, name):
        """The libraries a LINK_COMPONENTS word means."""
        key = name.lower()
        if key in ALL_TARGETS:
            return [t + ALL_TARGETS[key] for t in TARGETS if (t + ALL_TARGETS[key]).lower() in self.by_name]
        if key in [t.lower() for t in TARGETS]:
            return sorted({c.name for c in self.by_name.values() if c.group and c.group.lower() == key})
        if key in self.by_name:
            return [self.by_name[key].name]
        fail(f"no library or group called {name}")

    def closure(self, names):
        """`names` and everything they need, each once, those needed by
        others after them (the order a linker reads archives in)."""
        order, seen = [], set()

        def visit(name):
            for library in self.expand(name):
                library = self.by_name[library.lower()].name
                if library.lower() in seen:
                    continue
                seen.add(library.lower())
                for needed in self.by_name[library.lower()].links:
                    visit(needed)
                order.insert(0, library)
        for name in names:
            visit(name)
        return order

    def tablegen_directories(self, library):
        """The directories whose tablegen() output `library` includes:
        those its DEPENDS names, the intrinsics (which every library of
        LLVM may include), and the target's own when it is a part of a
        target (CMake adds a target's TableGen to everything beneath)."""
        component = self.by_name[library.lower()]
        found = [self.tablegen["intrinsics_gen"]]
        found += [self.tablegen[d] for d in component.depends if d in self.tablegen]
        m = re.match(r"(lib/Target/\w+)", component.directory)
        if m and m.group(1) in self.tablegen.values():
            found.append(m.group(1))
        return list(dict.fromkeys(found))


def tool_of(directory, name):
    """A tool's sources and LLVM_LINK_COMPONENTS, from its CMakeLists.txt."""
    for call, words, variables in expanded(cmake_calls(os.path.join(LLVM, directory, "CMakeLists.txt"))):
        if call in ("add_llvm_tool", "add_tablegen") and words and words[0] == name:
            return files(directory, sections(words[1:])[""]), variables.get("LLVM_LINK_COMPONENTS", [])
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


# The stack LLVM's tools are given: 8 MiB, what Clang asks of Linux
# (its parser and code generator recurse as deep as the program they
# read). The kernel gives 64 KiB to a program that does not ask.
STACK = 8 << 20


def set_stack(path, size):
    """Writes `size` into the program's PT_GNU_STACK header, where the
    kernel looks for the stack a program wants. lld writes the header
    (user/cxx.ld names it) but, with the program headers listed in a
    linker script, leaves its size 0 whatever -z stack-size says."""
    with open(path, "r+b") as f:
        header = f.read(64)
        offset, = struct.unpack_from("<Q", header, 32)
        size_of, count = struct.unpack_from("<HH", header, 54)
        for n in range(count):
            f.seek(offset + n * size_of)
            kind, = struct.unpack("<I", f.read(4))
            if kind == 0x6474E551:  # PT_GNU_STACK
                f.seek(offset + n * size_of + 40)  # p_memsz
                f.write(struct.pack("<Q", size))
                return
    fail(f"{path} has no PT_GNU_STACK header to write its stack size in")


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
    for name in tools:
        if name not in TOOL_DIRECTORIES:
            fail("no such tool: " + name)
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
    components = Components()
    wanted = {}
    needed = []
    for name in tools:
        sources, links = tool_of(TOOL_DIRECTORIES[name], name)
        libraries = components.closure(links + EXTRA_LIBRARIES.get(name, []))
        wanted[name] = (sources, libraries)
        needed += [library for library in libraries if library not in needed]
    directories = [components.tablegen["intrinsics_gen"]]
    for library in needed:
        directories += [d for d in components.tablegen_directories(library) if d not in directories]
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
                       "-I", os.path.join(LLVM, "include"), "-I", os.path.join(LLVM, "lib/Target"),
                       td, "-o", target, "-d", depfile]
            code, text = run(command)
            if code:
                fail(f"llvm-tblgen failed on {output}:\n{text}")

    # 3. The libraries. Every generated directory is searched by every
    # compile, as CMake's include path does for a target's files.
    includes = ["-I", os.path.join(gen, "include"), "-I", os.path.join(LLVM, "include")]
    for directory in directories:
        includes += ["-I", os.path.join(gen, directory)]
    common = ["-D__STDC_CONSTANT_MACROS", "-D__STDC_FORMAT_MACROS", "-D__STDC_LIMIT_MACROS", "-DNDEBUG",
              "-ffunction-sections", "-fdata-sections"]
    work = []
    archives = {}

    def compile_command(source, directory):
        path = os.path.join(LLVM, source)
        if source.endswith(".c"):
            return ["clang", *cflags, *common, *BLAKE3_FLAGS, *includes]
        command = ["clang++", *cxxflags, *common, *includes, "-I", os.path.join(LLVM, directory)]
        # A target's files include each other from the target's top
        # ("MCTargetDesc/X86BaseInfo.h"), as CMake lets them.
        m = re.match(r"(lib/Target/\w+)", directory)
        if m:
            command += ["-I", os.path.join(LLVM, m.group(1)), "-I", os.path.join(gen, m.group(1))]
        if directory.startswith("utils/TableGen"):
            command += ["-I", os.path.join(LLVM, "utils/TableGen")]
        return command

    def objects_of(sources, directory):
        objects = []
        for source in sources:
            obj = os.path.join(out, "obj", re.sub(r"\.(cpp|c)$", ".o", source))
            objects.append(obj)
            depfile = obj[:-2] + ".d"
            deps = dependencies(depfile)
            if deps is not None and not newer(obj, deps):
                continue
            command = compile_command(source, directory) + ["-MD", "-MF", depfile, "-c", os.path.join(LLVM, source),
                                                            "-o", obj]
            work.append((source, obj, command))
        return objects

    for library in needed:
        component = components.by_name[library.lower()]
        sources = list(component.sources)
        if library == "Support":
            sources += [os.path.join(component.directory, f) for f in BLAKE3]
        archives[library] = (os.path.join(out, "lib", "LLVM" + library + ".a"),
                             objects_of(sources, component.directory))

    # The tools' own sources.
    tool_objects = {name: objects_of(wanted[name][0], TOOL_DIRECTORIES[name]) for name in tools}

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
        for l in wanted[tool][1]:
            if l in OBJECT_LIBRARIES:
                objects += archives[l][1]
        libraries = [archives[l][0] for l in wanted[tool][1] if l not in OBJECT_LIBRARIES]
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
        set_stack(output, STACK)


if __name__ == "__main__":
    main()
