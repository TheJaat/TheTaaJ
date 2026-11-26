#!/usr/bin/env python3
"""lanes-check.py - is every open work item in exactly one lane?

    python3 tools/lanes-check.py

Reads the item tables of docs/who-does-what.md (rows whose first cell is
an item such as K60, J59 or E1; an item is done when its Notes say
"**Done") and the lane tables of docs/lanes.md (the first cell of each
row under a "### `lane`" heading), and says:

  - open items that are in no lane, and not named under "Not in a lane";
  - items that are in two lanes, or twice in one;
  - items in a lane that are done, or that who-does-what.md doesn't have.

Exits 1 if it found any of these, so it can run before a merge.
"""

import re
import sys

ITEM = re.compile(r"^[KJE]\d+$")


def expand(text):
    """The items a piece of text names: K60, and ranges such as E6-E8."""
    names = set()
    for first, letter, last in re.findall(r"\b([KJE]\d+)(?:-([KJE])?(\d+))?\b", text):
        if not last:
            names.add(first)
            continue
        start = int(first[1:])
        for n in range(start, int(last) + 1):
            names.add(first[0] + str(n))
    return names


def cells(line):
    return [c.strip() for c in line.strip().strip("|").split("|")]


def items(path):
    """{item: done?} from the work list's tables."""
    found = {}
    for line in open(path, encoding="utf-8"):
        if not line.startswith("| "):
            continue
        row = cells(line)
        if ITEM.match(row[0]):
            done = "**Done" in row[-1]
            # An item listed twice (as K107 is, in Eras 18 and 21) is
            # open if either row says so.
            found[row[0]] = found.get(row[0], True) and done
    return found


def lanes(path):
    """({lane: [items]}, {items not in a lane})."""
    lane, table, outside, reading_outside = None, {}, set(), False
    for line in open(path, encoding="utf-8"):
        heading = re.match(r"^### `([a-z0-9-]+)`", line)
        if heading:
            lane = heading.group(1)
            table[lane] = []
            continue
        if line.startswith("## "):
            lane = None
        if line.startswith("**Not in a lane"):
            reading_outside = True
        elif reading_outside and not line.strip():
            reading_outside = False
        if reading_outside:
            outside |= expand(line)
        if lane and line.startswith("| "):
            first = cells(line)[0]
            if ITEM.match(first):
                table[lane].append(first)
    return table, outside


def main():
    work = items("docs/who-does-what.md")
    table, outside = lanes("docs/lanes.md")
    problems = []

    where = {}
    for lane, names in table.items():
        for name in names:
            where.setdefault(name, []).append(lane)

    for name, places in sorted(where.items()):
        if len(places) > 1:
            problems.append(f"{name} is in more than one place: {', '.join(places)}")
        if name not in work:
            problems.append(f"{name} (lane {places[0]}) is not in docs/who-does-what.md")
        elif work[name]:
            problems.append(f"{name} (lane {places[0]}) is done: take it out of the lane")

    for name, done in sorted(work.items(), key=lambda kv: (kv[0][0], int(kv[0][1:]))):
        if not done and name not in where and name not in outside:
            problems.append(f"{name} is open and in no lane")

    open_items = sum(1 for done in work.values() if not done)
    in_lanes = sum(len(v) for v in table.values())
    print(f"{open_items} open items; {in_lanes} rows in {len(table)} lanes; "
          f"{len(outside & set(work))} named as not in a lane")
    for p in problems:
        print("  " + p)
    if problems:
        sys.exit(1)
    print("lanes-check: every open item is in one lane")


if __name__ == "__main__":
    main()
