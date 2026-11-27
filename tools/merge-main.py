#!/usr/bin/env python3
"""merge-main.py [REF] - merges main (origin/main, or REF) into the branch
this tree is on, and settles by itself the conflicts that are only
bookkeeping (journal entry 132):

  1. **Numbers given at another merge.** A branch stacked on another (rule
     2; the night queue) has that one's lines with `NNN` in them - "journal
     entry 132", "**Done (entry 132)**" - and once that one is merged, main
     has the same lines numbered (135). Where this branch changed lines
     next to them, git sees two changes to one place. Here, for each file
     in conflict: every line of the merge base with `NNN` that main has
     with a number instead is numbered the same way in the base and in
     this branch too, and the file merged again - main's change is then
     already in both, and only real changes are left.
  2. **The work list split** (entry 132): a conflict in
     docs/who-does-what.md is handed to tools/work-list-move.py.

Anything else in conflict is left as git left it, and said. Run from the
item's tree; TheKernel too, if it has a branch of the same name (its
main merged in first, the same way, and `kernel` pointed at the result).
Then test (rule 15) and commit: it does not commit, so you see it first.
"""
import os
import re
import subprocess
import sys

ROOT = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
MARK = "NN" + "N"  # in two pieces: this file's own lines are not numbered at its merge
PLACEHOLDER = re.compile(r"(?<![`\w])" + MARK + r"(?![-\w`])")


def git(*args, cwd=ROOT, check=True):
    done = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and done.returncode != 0:
        sys.exit(f"merge-main: git {' '.join(args)}: {done.stderr.strip()}")
    return done


def stage(cwd, n, path):
    done = git("show", f":{n}:{path}", cwd=cwd, check=False)
    return done.stdout if done.returncode == 0 else None


def numbering(base, theirs):
    """{base line: main's numbered line} for each base line with 132 that
    main has, word for word, with a number in 132's place."""
    found = {}
    theirs_lines = set(theirs.split("\n"))
    for line in set(base.split("\n")):
        parts = PLACEHOLDER.split(line)
        if len(parts) < 2:
            continue
        pattern = re.compile("^" + r"\d+".join(re.escape(part) for part in parts) + "$")
        matches = [t for t in theirs_lines if pattern.match(t)]
        if len(matches) == 1:
            found[line] = matches[0]
    return found


def renumbered(text, mapping):
    return "\n".join(mapping.get(line, line) for line in text.split("\n"))


def settle(cwd):
    """Settles what it can in the stopped merge in `cwd`; returns the
    files still in conflict."""
    left = []
    conflicted = git("diff", "--name-only", "--diff-filter=U", cwd=cwd).stdout.split()
    for path in conflicted:
        if path in ("kernel", "bootloader"):
            left.append(path)
            continue
        if path == "docs/who-does-what.md" and os.path.exists(os.path.join(cwd, "tools/work-list-move.py")):
            moved = subprocess.run(["python3", "tools/work-list-move.py"], cwd=cwd, capture_output=True, text=True)
            print(moved.stdout.strip())
            if moved.returncode != 0:
                left.append(path)
            continue
        base, ours, theirs = stage(cwd, 1, path), stage(cwd, 2, path), stage(cwd, 3, path)
        if base is None or ours is None or theirs is None:
            left.append(path)
            continue
        mapping = numbering(base, theirs)
        if not mapping:
            left.append(path)
            continue
        scratch = os.path.join(cwd, ".git-merge-main")
        os.makedirs(scratch, exist_ok=True)
        names = {}
        for name, text in (("ours", renumbered(ours, mapping)), ("base", renumbered(base, mapping)), ("theirs", theirs)):
            names[name] = os.path.join(scratch, name)
            open(names[name], "w").write(text)
        merged = subprocess.run(["git", "merge-file", "-p", "-L", "ours", "-L", "base", "-L", "theirs",
                                 names["ours"], names["base"], names["theirs"]], capture_output=True, text=True)
        for name in names.values():
            os.remove(name)
        os.rmdir(scratch)
        open(os.path.join(cwd, path), "w").write(merged.stdout)
        if merged.returncode == 0:
            git("add", path, cwd=cwd)
            print(f"merge-main: {path}: settled - {len(mapping)} line(s) numbered as main has them")
        else:
            left.append(path)
            print(f"merge-main: {path}: numbered as main has it, but a real conflict is left")
    return left


def merge(cwd, ref):
    git("fetch", "-q", "origin", cwd=cwd)
    done = git("merge", "--no-edit", "--no-commit", ref, cwd=cwd, check=False)
    if done.returncode == 0:
        return []
    if "CONFLICT" not in done.stdout + done.stderr:
        sys.exit(f"merge-main: the merge failed:\n{done.stdout}{done.stderr}")
    return settle(cwd)


def main():
    ref = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
    kernel = os.path.join(ROOT, "kernel")
    branch = git("branch", "--show-current").stdout.strip()
    left = []
    if branch and os.path.isdir(kernel) and git("branch", "--show-current", cwd=kernel).stdout.strip() == branch:
        k_left = merge(kernel, ref)
        if k_left:
            sys.exit(f"merge-main: TheKernel's merge has conflicts left: {' '.join(k_left)} - settle them there first")
        if git("rev-parse", "-q", "--verify", "MERGE_HEAD", cwd=kernel, check=False).returncode == 0:
            git("commit", "-q", "--no-edit", cwd=kernel)
        print(f"merge-main: TheKernel's {branch} has {ref} merged in")
    left = merge(ROOT, ref)
    if branch and os.path.isdir(kernel) and git("branch", "--show-current", cwd=kernel).stdout.strip() == branch:
        git("add", "kernel")
        left = [p for p in left if p != "kernel"]
    if left:
        print(f"merge-main: left for you: {' '.join(left)}")
        return 1
    print(f"merge-main: {ref} merged in, nothing left in conflict; test (rule 15), then `git commit`")
    return 0


if __name__ == "__main__":
    sys.exit(main())
