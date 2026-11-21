# Rebuilds docs/testing-notes.md from the "Testing note" at the end of
# each journal entry from 044 on.
import glob, re, os
out = ["# Testing notes, collected\n",
       "\nThe testing note from the end of each journal entry, from entry 044 on, one after another, so that a run of steps can be tested in one sitting. Each is copied from its entry word for word; the entry has the story. For entries before 044, see the entry itself.\n",
       "\nBuild once and everything below runs from the root of TheTaaJ. `make run64` opens a QEMU window to type in; the kernel's log appears in the terminal it was started from. `make test64` runs every automated test and should end `test64: all passed`.\n"]
for path in sorted(glob.glob("docs/journal/0[0-9][0-9]-*.md")):
    number = int(os.path.basename(path)[:3])
    if number < 44:
        continue
    text = open(path).read()
    title = text.split("\n", 1)[0].lstrip("# ").strip()
    note = text.split("\n## Testing note\n", 1)[1].strip()
    out.append(f"\n---\n\n## {number:03d} · {title}\n\n[The entry](journal/{os.path.basename(path)})\n\n{note}\n")
open("docs/testing-notes.md", "w").write("".join(out))
