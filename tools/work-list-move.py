#!/usr/bin/env python3
"""work-list-move.py - after docs/who-does-what.md was split (journal entry
NNN), carries a branch's changes to it into the files they now belong in.

A branch made before the split changed rows of who-does-what.md - an item
marked done, a new item, a note. Merging main into it, git finds those
lines gone from who-does-what.md (moved to docs/items/ or
docs/machines.md) and stops with a conflict. Run this then, in the middle
of the merge:

  git merge origin/main          # stops: conflict in docs/who-does-what.md
  python3 tools/work-list-move.py
  git diff --cached; git commit  # look, then finish the merge

It takes the branch's own changes to who-does-what.md (the merge base's
version against the branch's, from git's index), puts main's
who-does-what.md back, and makes each change again where its lines are
now: it looks for the lines the change replaced - and the line before
and after, for an addition - in who-does-what.md, docs/machines.md and
docs/items/*.md as main has them. Each change is made in exactly one
place, or not at all: a change whose lines are found nowhere, or in two
places, is printed for the session to make by hand, and the tool exits 1.
The files it changed are added to the index.
"""
import difflib
import glob
import os
import subprocess
import sys

ROOT = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
HUB = "docs/who-does-what.md"


def stage(n):
    done = subprocess.run(["git", "show", f":{n}:{HUB}"], cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"work-list-move: no conflict in {HUB} to work on (git show :{n}: failed) - is a merge stopped?")
    return done.stdout.split("\n")


def place(files, old, before, after, new):
    """Replace `old` (with `before` above it and `after` below, when old is
    empty) by `new`, in the one file that has it. Returns the file or why not."""
    found = []
    for path, lines in files.items():
        if old:
            n = len(old)
            for i in range(len(lines) - n + 1):
                if lines[i:i + n] == old:
                    found.append((path, i, i + n))
        else:
            for i in range(len(lines) + 1):
                if (not before or (i > 0 and lines[i - 1] == before)) and \
                   (not after or (i < len(lines) and lines[i] == after)) and (before or after):
                    found.append((path, i, i))
    if len(found) != 1:
        return None, f"found {'nowhere' if not found else f'in {len(found)} places'}"
    path, start, end = found[0]
    files[path][start:end] = new
    return path, None


def main():
    base, ours, theirs = stage(1), stage(2), stage(3)
    files = {HUB: theirs}
    for path in ["docs/machines.md"] + sorted(glob.glob(os.path.join(ROOT, "docs/items/*.md"))):
        path = os.path.relpath(os.path.join(ROOT, path), ROOT)
        if os.path.exists(os.path.join(ROOT, path)):
            files[path] = open(os.path.join(ROOT, path)).read().split("\n")
    changed, by_hand = set(), []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, base, ours, autojunk=False).get_opcodes():
        if tag == "equal":
            continue
        old, new = base[i1:i2], ours[j1:j2]
        before = base[i1 - 1] if i1 > 0 else None
        after = base[i2] if i2 < len(base) else None
        # A row's heading cells are no anchor: many tables have the same.
        if not old and before and before.startswith("|---"):
            before = None
        path, why = place(files, old, before, after, new)
        if path:
            changed.add(path)
        else:
            by_hand.append((why, old, new))
    for path in changed | {HUB}:
        open(os.path.join(ROOT, path), "w").write("\n".join(files[path]))
        subprocess.run(["git", "add", path], cwd=ROOT, check=True)
    for path in sorted(changed - {HUB}):
        print(f"work-list-move: changed {path}")
    if by_hand:
        print(f"work-list-move: {len(by_hand)} change(s) to make by hand ({HUB} is main's now):")
        for why, old, new in by_hand:
            print(f"  - {why}:")
            for line in old:
                print(f"      was: {line[:160]}")
            for line in new:
                print(f"      now: {line[:160]}")
        return 1
    print("work-list-move: every change made; look at `git diff --cached`, then commit the merge")
    return 0


if __name__ == "__main__":
    sys.exit(main())
