#!/usr/bin/env python3
"""night-test.py [QUEUE] - tests the queued branches, one after another,
and writes a report for the morning (journal entry 135).

By day, items are developed with quick checks only (their own boot test,
the host tests, `make unsafe-check`) and pushed to their branches. This
runs the full `make test64` on each, later - when the user says so - with
nobody waiting:

  QUEUE (default ~/.cache/taaj-night/queue): one branch a line, in the
  order to test (and to merge in the morning); `after BRANCH` stacks it on
  that one - an item that needs another not merged yet. `#` starts a
  comment.

For each branch, in its own tree (../TheTaaJ-night, made the first time):
  1. the branch, with TheTaaJ's main merged in - or, `after` another, that
     one's tested result; TheKernel's branch of the same name, if there is
     one, the same way, and `kernel` pointed at it;
  2. `make test64`, holding the machine's test64 lock (rule 13), with this
     machine's LLVM 18.1.3 settings if it has them (~/opt/llvm-18.1.3);
  3. passed: the merges pushed to the item's branches (TheKernel's first),
     so that in the morning merge-item.py can merge it as it is;
  4. a line of the report: passed or failed, the time, the parts that
     failed, the log. A branch that does not merge, or whose base failed,
     says so.

NIGHT_TEST="make unsafe-check" (say) runs that instead of make test64, to
try this script itself in minutes - and then nothing is pushed: only a
real make test64 makes a branch's merge fit to push.
"""
import datetime
import os
import re
import subprocess
import sys
import time

HOME = os.path.expanduser("~")
QUEUE = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HOME, ".cache/taaj-night/queue")
OUT = os.path.join(HOME, ".cache/taaj-night")
SOURCE = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
TREE = os.path.join(os.path.dirname(SOURCE), "TheTaaJ-night")
KERNEL = os.path.join(TREE, "kernel")
LLVM = os.path.join(HOME, "opt/llvm-18.1.3/env.sh")


def run(*command, cwd=TREE, check=True):
    done = subprocess.run(list(command), cwd=cwd, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RuntimeError(f"{' '.join(command)}: {done.stderr.strip()[:300]}")
    return done


def git(*args, cwd=TREE, check=True):
    return run("git", *args, cwd=cwd, check=check).stdout.strip()


def queue():
    entries = []
    for line in open(QUEUE):
        line = line.split("#", 1)[0].split()
        if not line:
            continue
        after = line[2] if len(line) == 3 and line[1] == "after" else None
        entries.append((line[0], after))
    return entries


def tree():
    """The night's own tree: a worktree of this repository, with the
    submodules and the cross compiler (as new-session.sh makes them)."""
    if not os.path.isdir(TREE):
        git("worktree", "add", "--detach", TREE, "origin/main", cwd=SOURCE)
        git("submodule", "update", "--init", "kernel", "bootloader")
        i686 = os.path.join(SOURCE, "toolchain/i686")
        if os.path.isdir(i686):
            git("submodule", "update", "--init", "toolchain")
            link = os.path.join(TREE, "toolchain/i686")
            if not os.path.exists(link):
                os.symlink(i686, link)


def prepare(branch, base_taaj, base_kernel):
    """Checks the branch out with its base merged in. Returns the TheKernel
    commit it uses, or raises with why it could not."""
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    git("reset", "-q", "--hard")
    git("checkout", "-q", "--detach", f"origin/{branch}")
    git("submodule", "update", "-q", "--init", "kernel", "bootloader")
    kernel_branch = git("ls-remote", "origin", f"refs/heads/{branch}", cwd=KERNEL)
    kernel = None
    if kernel_branch:
        git("checkout", "-q", "--detach", f"origin/{branch}", cwd=KERNEL)
        if run("git", "merge", "-q", "--no-edit", base_kernel, cwd=KERNEL, check=False).returncode != 0:
            git("merge", "--abort", cwd=KERNEL, check=False)
            raise RuntimeError(f"TheKernel's {branch} does not merge with {base_kernel[:10]}")
        kernel = git("rev-parse", "HEAD", cwd=KERNEL)
    merged = run("git", "merge", "-q", "--no-edit", base_taaj, check=False)
    if merged.returncode != 0:
        conflicts = git("diff", "--name-only", "--diff-filter=U").split()
        if conflicts != ["kernel"]:
            git("merge", "--abort", check=False)
            raise RuntimeError(f"does not merge with {base_taaj[:10]}: {' '.join(conflicts)}")
    if not kernel and merged.returncode != 0:
        # No TheKernel branch of its own, and the base moved the pointer:
        # the base's.
        kernel = git("ls-tree", base_taaj, "kernel").split()[2]
    if kernel:
        git("checkout", "-q", kernel, cwd=KERNEL)
        git("add", "kernel")
    if git("diff", "--cached", "--name-only") or merged.returncode != 0:
        git("commit", "-q", "--no-edit", "-m", f"Merge {base_taaj[:10]} into {branch} (night test)")
    # The merge may have moved `kernel` (the base's newer one): check out
    # what the result says, not what the branch had - or the programs are
    # built against an older ABI than the kernel's.
    if not kernel_branch:
        git("submodule", "update", "-q", "kernel")
    git("submodule", "update", "-q", "bootloader")
    return kernel or git("ls-tree", "HEAD", "kernel").split()[2]


def test(log):
    command = os.environ.get("NIGHT_TEST", "make --no-print-directory -k test64")
    setup = f". {LLVM} && make_flags=\"LLVM_TBLGEN=$LLVM_TBLGEN\" || make_flags=" if os.path.isfile(LLVM) else "make_flags="
    script = f"{setup}; flock /tmp/taaj-test64.lock sh -c \"{command} $make_flags\""
    start = time.time()
    with open(log, "w") as out:
        status = subprocess.run(["sh", "-c", script], cwd=TREE, stdout=out, stderr=subprocess.STDOUT).returncode
    failed = sorted({m for m in re.findall(r"FAIL[^:]*: (.{0,80})", open(log, errors="replace").read())})
    return status, int(time.time() - start), failed


def main():
    tree()
    night = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    logs = os.path.join(OUT, "logs", night)
    os.makedirs(logs, exist_ok=True)
    report = os.path.join(OUT, "reports", f"{night}.md")
    os.makedirs(os.path.dirname(report), exist_ok=True)
    lines = [f"# Night test {night}", "", "| Branch | Base | Result | Time | What failed | Log |", "|---|---|---|---|---|---|"]
    tested = {}  # branch -> (TheTaaJ commit, TheKernel commit), if it passed
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    for branch, after in queue():
        log = os.path.join(logs, f"{branch}.log")
        if after:
            base = tested.get(after)
            if not base:
                lines.append(f"| {branch} | after {after} | **not run**: {after} did not pass | | | |")
                continue
            base_taaj, base_kernel, said = base[0], base[1], f"{after}'s result"
        else:
            base_taaj, base_kernel, said = "origin/main", "origin/main", "main"
        if not git("ls-remote", "origin", f"refs/heads/{branch}"):
            lines.append(f"| {branch} | {said} | **not pushed**: no such branch on the server | | | |")
            continue
        print(f"night-test: {branch} on {said}", flush=True)
        try:
            kernel = prepare(branch, base_taaj, base_kernel)
        except RuntimeError as why:
            lines.append(f"| {branch} | {said} | **does not merge** | | {why} | |")
            continue
        status, seconds, failed = test(log)
        if status == 0:
            # Kept for the branches stacked on this one, and pushed, so the
            # morning merge finds the base already merged in.
            tested[branch] = (git("rev-parse", "HEAD"), kernel)
            pushed = []
            if os.environ.get("NIGHT_TEST"):
                pass  # a stand-in test: nothing is pushed
            elif git("ls-remote", "origin", f"refs/heads/{branch}", cwd=KERNEL):
                pushed.append(run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", cwd=KERNEL, check=False).returncode)
            if not os.environ.get("NIGHT_TEST"):
                pushed.append(run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", check=False).returncode)
            note = "" if not any(pushed) else " (the merge could not be pushed: the branch moved)"
            lines.append(f"| {branch} | {said} | **passed**{note} | {seconds // 60} min | | `{log}` |")
        else:
            lines.append(f"| {branch} | {said} | **FAILED** ({status}) | {seconds // 60} min | {'; '.join(failed[:5]) or 'see the log'} | `{log}` |")
        open(report, "w").write("\n".join(lines) + "\n")
    open(report, "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nnight-test: the report is {report}")


main()
