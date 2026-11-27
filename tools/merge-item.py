#!/usr/bin/env python3
"""merge-item.py "README ROW" - merges the item on this branch (rules 2, 5
and 12; start-here.md section 6, steps 6 to 8).

Run it from the session's tree, on the item's branch (and TheKernel on a
branch of the same name, if the item changed TheKernel), once the item is
tested with main merged in and the user has said yes. "README ROW" is the
row's text for docs/journal/README.md: what the entry did, in a sentence or
three, as the rows above it.

It does, stopping at the first thing that fails:

  1. checks: both trees clean; one journal entry named NNN-*.md; each
     branch has its main merged in (if not: merge, test, run again);
  2. the next free entry number N (from main's README), and the next free
     K and J numbers for new items (K?-word, J?-word);
  3. TheKernel first: `NNN` becomes N on the lines this branch added; that
     committed, merged into TheKernel's main, pushed. A refused push - one
     session merged first - undoes the commit and stops, with nothing of
     TheTaaJ's changed (journal entry 124 is why);
  4. TheTaaJ: the entry renamed and numbered, `NNN` on its added lines, the
     new items numbered, the README row, the testing notes, the lanes
     check; `kernel` pointed at what was just pushed; committed, merged
     into main, pushed - again undone and stopped if refused. The README
     row's commits are the item's own last commit (and TheKernel's main):
     known before the merge, so the row is written in the merge's own
     commit - one commit per merge, not two (entry NNN);
  5. the branches deleted, here and on the server.

Only lines this branch added are renumbered: the rules in
who-does-what.md and start-here.md say "entry `NNN`" as instructions, and
stay as they are.
"""
import os
import re
import subprocess
import sys

ROOT = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
KERNEL = os.path.join(ROOT, "kernel")


def git(*args, cwd=ROOT, check=True):
    done = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and done.returncode != 0:
        stop(f"git {' '.join(args)} failed:\n{done.stderr.strip()}")
    return done.stdout.strip()


def is_ancestor(older, newer, cwd=ROOT):
    return subprocess.run(["git", "merge-base", "--is-ancestor", older, newer], cwd=cwd).returncode == 0


# `NNN` as a number to give - not where it names the convention, as in
# `NNN-name.md` (in backticks, or followed by a hyphen).
# (Spelled in two pieces here: this file is merged by itself, and its own
# pattern must not be numbered.)
MARK = "NN" + "N"
PLACEHOLDER = re.compile(r"(?<![`\w])" + MARK + r"(?![-\w`])")


def stop(why):
    sys.exit(f"merge-item: {why}")


def say(what):
    print(f"merge-item: {what}")


def added_lines(cwd, base):
    """{file: [lines this branch added]} against `base`."""
    out = git("diff", "-U0", f"{base}...HEAD", cwd=cwd)
    files, current = {}, None
    for line in out.splitlines():
        if line.startswith("+++ "):
            current = None if line == "+++ /dev/null" else line[6:]
            if current:
                files.setdefault(current, [])
        elif line.startswith("+") and current:
            files[current].append(line[1:])
    return files


def renumber(cwd, base, number, also=()):
    """`NNN` becomes `number`, and each (old, new) pair of `also` is replaced,
    on the lines this branch added - nowhere else: the rules name `K?-hubs`
    and say "entry `NNN`" as examples. The files changed."""
    changed = []
    for name, lines in added_lines(cwd, base).items():
        path = os.path.join(cwd, name)
        if not os.path.isfile(path):
            continue
        try:
            text = open(path).read()
        except UnicodeDecodeError:
            continue
        new = text
        for line in set(lines):
            numbered = PLACEHOLDER.sub(str(number), line)
            for old, replacement in also:
                numbered = numbered.replace(old, replacement)
            if numbered != line:
                new = new.replace(line, numbered)
        if new != text:
            open(path, "w").write(new)
            changed.append(name)
    return changed


def main():
    if len(sys.argv) != 2 or not sys.argv[1].strip():
        stop('usage: merge-item.py "README ROW" (the row\'s text)')
    row_text = sys.argv[1].strip()
    branch = git("branch", "--show-current")
    if not branch or branch == "main":
        stop("not on an item's branch")

    # ---- 1. checks --------------------------------------------------------
    for cwd in (ROOT, KERNEL):
        dirty = [l for l in git("status", "--porcelain", cwd=cwd).splitlines() if not l.endswith(" bootloader")]
        if dirty:
            stop(f"{cwd} has changes not committed:\n" + "\n".join(dirty))
    entries = [f for f in os.listdir(os.path.join(ROOT, "docs/journal")) if f.startswith(MARK + "-") and f.endswith(".md")]
    if len(entries) != 1:
        stop(f"want one docs/journal/NNN-*.md, found {entries or 'none'}")
    entry = entries[0]
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    if not is_ancestor("origin/main", "HEAD"):
        stop("TheTaaJ's main has moved: merge it into the branch, test again (rule 15), and run this again")
    pointer_main = git("ls-tree", "origin/main", "kernel").split()[2]
    kernel_head = git("rev-parse", "HEAD", cwd=KERNEL)
    # To be pushed: TheKernel commits not on its main yet. To be pointed
    # at: whatever the branch has that main's pointer has not - after a
    # run that pushed TheKernel and then found TheTaaJ refused, the first
    # without the second.
    kernel_changed = not is_ancestor(kernel_head, "origin/main", KERNEL)
    kernel_moved = kernel_head != pointer_main
    if kernel_changed:
        if git("branch", "--show-current", cwd=KERNEL) != branch:
            stop(f"TheKernel has changes, but is not on a branch named {branch}")
        if not is_ancestor("origin/main", "HEAD", KERNEL):
            stop("TheKernel's main has moved: merge it into the branch, test again, and run this again")

    # ---- 2. numbers -------------------------------------------------------
    readme_main = git("show", "origin/main:docs/journal/README.md")
    number = max(int(n) for n in re.findall(r"^\| (\d+) \|", readme_main, re.M)) + 1
    # The work list: who-does-what.md and the item files (entry NNN).
    listed = ["docs/who-does-what.md"] + [f for f in git("ls-tree", "--name-only", "origin/main", "docs/items/").split()
                                          if f.endswith(".md")]
    work_main = "\n".join(git("show", f"origin/main:{f}") for f in listed)
    next_k = max([int(n) for n in re.findall(r"^\| K(\d+) \|", work_main, re.M)] or [0]) + 1
    next_j = max([int(n) for n in re.findall(r"^\| J(\d+) \|", work_main, re.M)] or [0]) + 1
    # New items: rows this branch added to the work list's tables that
    # begin `| K?-word |` or `| J?-word |`. Not every K?-word on an added
    # line - an entry or a script that explains the convention names some.
    items = []
    added = added_lines(ROOT, "origin/main")
    for line in [l for f, ls in added.items() if f == "docs/who-does-what.md" or f.startswith("docs/items/") for l in ls]:
        found = re.match(r"^\| ([KJ])\?-([a-z0-9-]+) \|", line)
        if found and found.groups() not in items:
            items.append(found.groups())
    renames = []
    for kind, word in items:
        if kind == "K":
            renames.append((f"`K?-{word}`", f"K{next_k}"))
            renames.append((f"K?-{word}", f"K{next_k}"))
            next_k += 1
        else:
            renames.append((f"`J?-{word}`", f"J{next_j}"))
            renames.append((f"J?-{word}", f"J{next_j}"))
            next_j += 1
    say(f"entry {number}" + (f"; new items: {', '.join(n for _, n in renames[1::2])}" if renames else ""))

    # ---- 3. TheKernel -----------------------------------------------------
    kernel_commit = kernel_head
    if kernel_changed:
        changed = renumber(KERNEL, "origin/main", number)
        if changed:
            git("add", *changed, cwd=KERNEL)
            git("commit", "-q", "-m", f"{branch}: TheTaaJ's journal entry is {number}", cwd=KERNEL)
        git("switch", "-q", "main", cwd=KERNEL)
        git("merge", "-q", "--ff-only", "origin/main", cwd=KERNEL)
        git("merge", "-q", "--ff-only", branch, cwd=KERNEL)
        pushed = subprocess.run(["git", "push", "-q", "origin", "main"], cwd=KERNEL, capture_output=True, text=True)
        if pushed.returncode != 0:
            git("reset", "-q", "--hard", "origin/main", cwd=KERNEL)
            git("switch", "-q", branch, cwd=KERNEL)
            if changed:
                git("reset", "-q", "--hard", "HEAD~1", cwd=KERNEL)
            stop("TheKernel's push was refused - another session merged first. Nothing of TheTaaJ's was changed. "
                 "Merge TheKernel's main into the branch, test again, and run this again")
        kernel_commit = git("rev-parse", "HEAD", cwd=KERNEL)
        say(f"TheKernel's main is {kernel_commit[:7]}")

    # ---- 4. TheTaaJ -------------------------------------------------------
    numbered = entry.replace(MARK + "-", f"{number}-", 1)
    git("mv", f"docs/journal/{entry}", f"docs/journal/{numbered}")
    path = os.path.join(ROOT, "docs/journal", numbered)
    text = PLACEHOLDER.sub(str(number), open(path).read())
    for old, new in renames:
        text = text.replace(old, new)
    open(path, "w").write(text)
    # Every file whose added lines it numbered - a script's comment saying
    # "journal entry NNN" too, not only docs/ (until entry NNN, those were
    # numbered but left out of the commit).
    numbered_files = renumber(ROOT, "origin/main", number, renames)
    readme = os.path.join(ROOT, "docs/journal/README.md")
    lines = open(readme).read().split("\n")
    last = max(i for i, l in enumerate(lines) if re.match(r"^\| \d+ \|", l))
    # The item's own last commit: in main's history once this is pushed,
    # and known now - the numbering commit's own name is not.
    said = f"`{git('rev-parse', 'HEAD')[:8]}`" + (f" (TheKernel `{kernel_commit[:7]}`)" if kernel_moved else "")
    lines.insert(last + 1, f"| {number} | [{row_text}]({numbered}) | written | {said} |")
    open(readme, "w").write("\n".join(lines))
    subprocess.run(["python3", "tools/testing-notes.py"], cwd=ROOT, check=True)
    lanes = subprocess.run(["python3", "tools/lanes-check.py"], cwd=ROOT, capture_output=True, text=True)
    if lanes.returncode != 0:
        say(lanes.stdout + lanes.stderr)
        stop("lanes-check failed (above); the numbering is done but not committed - fix the lanes, commit, and run this again")
    if kernel_moved:
        git("checkout", "-q", kernel_commit, cwd=KERNEL)
        git("add", "kernel")
    git("add", "-A", "docs")
    if numbered_files:
        git("add", *numbered_files)
    git("commit", "-q", "-m", f"{branch}, entry {number}: the journal number given at merge; README row; testing notes rebuilt"
        + (f"; the kernel at TheKernel's main ({kernel_commit[:7]})" if kernel_moved else ""))
    git("switch", "-q", "--detach", "HEAD")
    head = git("rev-parse", "HEAD")
    if not is_ancestor("origin/main", head):
        stop("TheTaaJ's main moved while this ran")
    pushed = subprocess.run(["git", "push", "-q", "origin", f"{head}:refs/heads/main"], cwd=ROOT, capture_output=True, text=True)
    if pushed.returncode != 0:
        git("switch", "-q", branch)
        git("reset", "-q", "--hard", "HEAD~1")
        git("submodule", "update", "-q", "kernel")
        if kernel_changed:
            # TheKernel's half is on its main now: the branch says so, and
            # the next run points TheTaaJ at it.
            git("switch", "-q", branch, cwd=KERNEL)
            git("add", "kernel")
            git("commit", "-q", "-m", f"{branch}: the kernel at TheKernel's main ({kernel_commit[:7]}), merged there before TheTaaJ's push was refused")
        stop(f"TheTaaJ's push was refused - another session merged first. Its numbering is undone"
             + (f"; TheKernel's main already has this item ({kernel_commit[:7]}), its comments saying entry {number} - "
                f"if that number goes to the other session, correct them with the next merge; this run will point at it"
                if kernel_changed else "")
             + ". Merge TheTaaJ's main into the branch, test again, and run this again")
    say(f"TheTaaJ's main is {head[:8]}")

    # ---- 5. the branches --------------------------------------------------
    for cwd in (ROOT, KERNEL) if kernel_moved else (ROOT,):
        subprocess.run(["git", "push", "-q", "origin", "--delete", branch], cwd=cwd, capture_output=True)
        subprocess.run(["git", "branch", "-q", "-D", branch], cwd=cwd, capture_output=True)
    git("submodule", "update", "-q", "kernel")
    say(f"entry {number} merged: {said}")


main()
