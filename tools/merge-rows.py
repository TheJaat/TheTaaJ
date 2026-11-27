#!/usr/bin/env python3
"""merge-rows.py BASE OURS THEIRS [PATH] - git's merge driver for the work
lists (journal entry NNN): table rows merged one item at a time.

Two branches that change two different rows of one table - J92 and J94,
one under the other - are not a conflict. But git merges lines, and two
changes on neighbouring lines are one conflict to it. With fourteen
sessions changing their items' rows ("Done (entry NNN)", a note), that was
the most common conflict of all.

So, where git's own merge (`git merge-file`) leaves a conflict and every
line of it, on all three sides, is a table row whose first cell names an
item (`| K187 |`, `| J?-word |`, `| U8 |`), the rows are merged by that
name:

  - a row only one side changed takes that side's;
  - a row both changed the same way, or neither, is kept;
  - a row only one side added is added, where that side has it;
  - a row one side removed and the other did not change is removed;
  - a row both changed differently stays a conflict - that one row, with
    git's markers around it - for the session to decide.

Anything else (prose, a heading) stays git's conflict, untouched.
Writes the result to OURS; exits 0 if nothing is left in conflict, 1 if
something is (git then stops the merge, as with any conflict).

Set up by tools/new-session.sh (or by hand, once per clone):

  git config merge.rows.name "work-list rows, one item at a time"
  git config merge.rows.driver "python3 tools/merge-rows.py %O %A %B %P"

and .gitattributes names the files. Without that config, git merges them
as text, as before: nothing breaks, only more conflicts.
"""
import re
import subprocess
import sys

ROW = re.compile(r"^\| ([A-Z][A-Za-z]?[0-9?][\w?-]*) \|")


def key(line):
    found = ROW.match(line)
    return found.group(1) if found else None


def merge_rows(base, ours, theirs):
    """Lists of lines -> (merged lines, conflicts left)."""
    if not all(key(l) for l in base + ours + theirs):
        return None
    b = {key(l): l for l in base}
    o = {key(l): l for l in ours}
    t = {key(l): l for l in theirs}
    order = []
    # Ours' order, and each row only theirs has after the row before it there.
    for k in [key(l) for l in ours]:
        order.append(k)
    previous = None
    for k in [key(l) for l in theirs]:
        if k not in order:
            at = order.index(previous) + 1 if previous in order else len(order)
            order.insert(at, k)
        previous = k
    out, left = [], 0
    for k in order:
        bl, ol, tl = b.get(k), o.get(k), t.get(k)
        if ol == tl:
            choice = [ol] if ol is not None else []
        elif ol == bl:
            choice = [tl] if tl is not None else []
        elif tl == bl:
            choice = [ol] if ol is not None else []
        else:
            left += 1
            choice = ["<<<<<<< ours", ol or "(removed)", "=======", tl or "(removed)", ">>>>>>> theirs"]
        out += choice
    return out, left


def main():
    base_path, ours_path, theirs_path = sys.argv[1:4]
    done = subprocess.run(["git", "merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs",
                           ours_path, base_path, theirs_path], capture_output=True, text=True)
    if done.returncode == 0:
        open(ours_path, "w").write(done.stdout)
        return 0
    if done.returncode < 0 or done.returncode > 127:
        return 2
    text = done.stdout
    trailing = text.endswith("\n")
    lines = text.split("\n")
    if trailing:
        lines = lines[:-1]
    out, left, i = [], 0, 0
    while i < len(lines):
        if not lines[i].startswith("<<<<<<< "):
            out.append(lines[i])
            i += 1
            continue
        # <<<<<<< ours / ||||||| base / ======= / >>>>>>> theirs
        j = i + 1
        ours = []
        while not lines[j].startswith("||||||| "):
            ours.append(lines[j]); j += 1
        j += 1
        base = []
        while lines[j] != "=======":
            base.append(lines[j]); j += 1
        j += 1
        theirs = []
        while not lines[j].startswith(">>>>>>> "):
            theirs.append(lines[j]); j += 1
        merged = merge_rows(base, ours, theirs)
        if merged is None:
            out += lines[i:j + 1]
            left += 1
        else:
            out += merged[0]
            left += merged[1]
        i = j + 1
    open(ours_path, "w").write("\n".join(out) + ("\n" if trailing else ""))
    return 1 if left else 0


if __name__ == "__main__":
    sys.exit(main())
