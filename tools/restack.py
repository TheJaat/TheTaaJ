#!/usr/bin/env python3
"""restack.py [--check] [QUEUE] - keeps a stack of branches stacked
(journal entry 139).

The night queue stacks branches in merge order: `b after a` means b is
tested, and merged, on a's result, so b must have a in it. After a fix to
a - a test failed, a review found something - every branch above it
must take it in again, in order, and each one's `kernel` pointer must
stay right: its own TheKernel branch's commit if it has one, else the one
below it's. Done by hand, that is a loop of merges and pushes, and one
careless commit puts a submodule checked out for another branch into a
pointer (it happened, entry 139).

For each `b after a` of QUEUE (default ~/.cache/taaj-night/queue), in
order, in a tree of its own (../TheTaaJ-restack, made the first time):
  1. b's branch, with a's merged in if it does not have it yet -
     tools/merge-main.py settles the bookkeeping conflicts; TheKernel's b
     the same way with TheKernel's a, when both have one;
  2. `kernel` set to b's TheKernel branch, or else to a's pointer;
  3. pushed, TheKernel's first.
A real conflict stops it there, and says which files.

--check only says what it would do: which branches lack the one below,
and which pointers are wrong.
"""
import os
import subprocess
import sys

HOME = os.path.expanduser("~")
ARGS = [a for a in sys.argv[1:] if a != "--check"]
CHECK = "--check" in sys.argv[1:]
QUEUE = ARGS[0] if ARGS else os.path.join(HOME, ".cache/taaj-night/queue")
SOURCE = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
TREE = os.path.join(os.path.dirname(SOURCE), "TheTaaJ-restack")
KERNEL = os.path.join(TREE, "kernel")


def git(*args, cwd=None, check=True):
    done = subprocess.run(["git", *args], cwd=cwd or TREE, capture_output=True, text=True)
    if check and done.returncode != 0:
        sys.exit(f"restack: git {' '.join(args)}: {done.stderr.strip()[:300]}")
    return done.stdout.strip()


def has(ref, cwd=None):
    return subprocess.run(["git", "rev-parse", "-q", "--verify", ref], cwd=cwd or TREE, capture_output=True).returncode == 0


def ancestor(older, newer, cwd=None):
    return subprocess.run(["git", "merge-base", "--is-ancestor", older, newer], cwd=cwd or TREE).returncode == 0


def main():
    if not os.path.isdir(TREE):
        git("worktree", "add", "-q", "--detach", TREE, "origin/main", cwd=SOURCE)
        git("submodule", "update", "-q", "--init", "kernel")
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    pairs = []
    for line in open(QUEUE):
        words = line.split("#", 1)[0].split()
        if len(words) == 3 and words[1] == "after":
            pairs.append((words[0], words[2]))
    if not pairs:
        print("restack: no `b after a` lines in the queue")
        return 0
    stale = set()  # --check: branches below that will take something in
    for branch, below in pairs:
        own_kernel = has(f"origin/{branch}", KERNEL)
        below_kernel = has(f"origin/{below}", KERNEL)
        need_merge = not ancestor(f"origin/{below}", f"origin/{branch}")
        need_kmerge = own_kernel and below_kernel and not ancestor(f"origin/{below}", f"origin/{branch}", KERNEL)
        if CHECK:
            want = git("rev-parse", f"origin/{branch}", cwd=KERNEL) if own_kernel else git("ls-tree", f"origin/{below}", "kernel").split()[2]
            have = git("ls-tree", f"origin/{branch}", "kernel").split()[2]
            said = []
            if need_merge:
                said.append(f"lacks {below}")
            elif below in stale:
                said.append(f"will take in what {below} takes in")
            if need_kmerge:
                said.append(f"TheKernel's lacks {below}'s")
            if not need_merge and have != want:
                said.append(f"kernel pointer {have[:8]}, wants {want[:8]}")
            if said:
                stale.add(branch)
            print(f"restack: {branch}: {', '.join(said) or 'stacked'}")
            continue
        git("reset", "-q", "--hard")
        git("checkout", "-q", "--detach", f"origin/{branch}")
        if need_kmerge:
            git("checkout", "-q", "--detach", f"origin/{branch}", cwd=KERNEL)
            if subprocess.run(["git", "merge", "-q", "--no-edit", f"origin/{below}"], cwd=KERNEL, capture_output=True).returncode:
                git("merge", "--abort", cwd=KERNEL, check=False)
                sys.exit(f"restack: TheKernel's {branch} does not merge with {below}'s: settle it by hand")
            git("push", "-q", "origin", f"HEAD:refs/heads/{branch}", cwd=KERNEL)
            git("fetch", "-q", "origin", cwd=KERNEL)
        if need_merge:
            done = subprocess.run(["python3", "tools/merge-main.py", f"origin/{below}"], cwd=TREE, capture_output=True, text=True)
            if done.returncode:
                print(done.stdout + done.stderr)
                git("merge", "--abort", check=False)
                sys.exit(f"restack: {branch} does not take {below} in by itself (above): settle it by hand")
        # The pointer: its own TheKernel branch, else the one below's.
        want = git("rev-parse", f"origin/{branch}", cwd=KERNEL) if own_kernel else git("ls-tree", f"origin/{below}", "kernel").split()[2]
        git("update-index", "--cacheinfo", f"160000,{want},kernel")
        merging = has("MERGE_HEAD")
        if merging or git("diff", "--cached", "--name-only"):
            message = f"Merge {below} into {branch} (restack)" if merging else f"{branch}: the kernel pointer at {want[:8]} (restack)"
            git("commit", "-q", "--no-edit", "-m", message)
            git("push", "-q", "origin", f"HEAD:refs/heads/{branch}")
            git("fetch", "-q", "origin")
            print(f"restack: {branch}: {message}, pushed")
        else:
            print(f"restack: {branch}: already stacked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
