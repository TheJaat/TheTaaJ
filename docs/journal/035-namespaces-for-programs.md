# Namespaces for Programs: `ls` Asks for "files", and the Shell Decides What That Means

*Journal entry #35 · a client can ask the registry for a narrower namespace and hand it to a program; the `FILES` startup flag goes; "read-only" becomes a matter of what a name means*

The second half of the registry. Entry #34 moved the wiring *between services* to names: the shell asks for `"display"`, the file service asks for `"disk"`. But the programs the shell starts — `ls`, `cat`, `write` — were still handed the file service the old way, as a flagged startup handle the shell copied from its own.

After this entry, a program gets one thing for all its services: a channel to the registry, on which the names it was given exist.

```
taaj:/> ls
[registry] shell gave ls: files=files.look

taaj:/> write notes hello
[registry] shell gave write: files

taaj:/> with readonly given
[registry] shell gave given: files=files.look
given: it may ask for: files
given: the screen, and the files (38 of them), to read only
```

(The `[registry]` lines are in the log, not on the screen.)

---

## The problem with what there was

The shell held two channels to the file service — one that can change files, one that can only read (entry #21) — and for each program it copied the right one and passed it along with the startup flag `FILES`. That worked. Three things were wrong with it:

1. **One flag per service.** `FILES` was a bit in the startup flags with a fixed position among the handles. A second service a program might use — the display, a network stack one day — would need a second bit, a third position, and a change in the kernel's ABI, in init, in the shell and in the runtime. The mechanism did not grow.
2. **Two mechanisms.** Services found each other by name through the registry; programs found services by flag. Two ways to do the same thing means two things to understand and secure.
3. **The shell needed to hold everything it might give.** To hand a program a channel the shell had to have that very channel in its hand, copyable.

## Narrowing

One new request on a client's channel to the registry:

```
[NARROW] [label] 0 [new] 0 [old] 0 [new] 0 [old] 0 ...
         reply: [OK] + the sending end of a channel to a new namespace
```

"Make me a namespace. In it there shall be a name `new`, and it shall mean what `old` means in mine." The registry makes a channel, keeps the receiving end — and from then on answers it like any other client's — and returns the sending end. The asker passes that to the program it starts.

```
   shell's namespace                       ls's namespace
   ─────────────────                       ──────────────
   keys                                    
   display                                 
   init                                    
   system                                  
   files       ──► the read-write channel  
   files.look  ──► the read-only channel ◄── files
   disk.look                               
```

### The one rule: you cannot give what you cannot find

Every `old` must be a name in the asker's *own* namespace. If one is not, the whole request is refused:

```rust
let found = from.names.iter().find(|(name, _)| name == old);
match found {
    Some((_, service)) if valid_name(new) => meant.push((String::from(new), service.clone())),
    _ => return Err(channel),
}
```

That single check is what keeps this safe. A namespace can only be made *from* a namespace, by taking names out of it or renaming them. Authority can be passed on and cut down; it cannot be made. A program handed a narrowed namespace can narrow it again for something it starts, and that one can only be narrower still. The host test:

```rust
// The disk is registered - but not in the shell's namespace.
assert_eq!(table.narrow(0, "sneak", &[("disk", "disk")], 300), Err(300));
```

### Renaming is the point

It would have been simpler to allow only subsets: "the same names, fewer". Renaming looks like a frill. It is the most useful part.

Every program that works with files calls `taaj::file_service()`, which now does:

```rust
let channel = services()?.lookup("files").ok()?;
```

`ls` asks for `"files"`. `write` asks for `"files"`. Neither knows there are two channels, or which it has. The *shell* decides, in one line:

```rust
let names: &[(&str, &str)] = match grant {
    Grant::Files                              => &[("files", "files")],
    Grant::ReadOnly                           => &[("files", "files.look")],
    Grant::Usual if on_disk(CHANGE_PROGRAMS)  => &[("files", "files")],
    Grant::Usual if on_disk(LOOK_PROGRAMS)    => &[("files", "files.look")],
    Grant::Usual                              => &[],
};
```

**What a program may do is decided by what its names mean, not by which names it knows to ask for.** `with readonly write notes x` runs the very same `write`, asking for the very same `"files"`, and gets a channel on which its request is refused.

Without renaming, `ls` would have to ask for `"files.look"` — it would have to *know* it is a read-only program, and I would have to trust that it asks for the weaker thing. With renaming the program cannot ask for more, because in its world there is nothing more: one name, `files`, and no hint that another channel exists.

This is the same idea as standard output in entry #31, arriving from the other side. There a program wrote to "its output" without knowing what that was. Here it asks for "the files" without knowing which. In both, the one who starts the program chooses, and the program is the same program whatever was chosen.

### What a narrowed name records

A detail in the table. When the shell says `files = files.look`, the new namespace does not store "look up `files.look` in the shell's namespace". It stores the *service* that name meant at that moment:

```rust
names: Vec<(String, String)>,     // (the name here, the service it means)
```

So the program's namespace does not depend on the shell's continuing to exist or to stay the same. If the shell is restarted, programs it started keep what they were given. A chain of "ask my parent" would break at the first link to die.

### How long a namespace lasts

Nobody deletes one. The registry holds the receiving end of the channel; the program holds the sending end. When the program exits its handles close, the registry's next receive on that channel says "every sender is gone", and the registry forgets it. The lifetime of the namespace *is* the lifetime of the handle — the same trick as the end of a stream (entry #31) and the end of a pipe (entry #32). This system keeps being able to answer "when is it over?" with "when the channel closes", and every time it means one less protocol message and one less thing to leak.

## What went away

- The startup flag **`FILES`** is now **`SERVICES`**: not "the file service's channel" but "your channel to the registry". Same bit, same place — and it will not need a sibling. The next service a program can be given is a line in the shell, not a change to the ABI.
- The shell no longer copies its own file channels for programs. It still holds them, for its own use (`cd`, `>`); what it gives a program is a *name*, and the registry makes the copy.
- `given` reports the names it found, so the mechanism can be seen:
  ```
  given: it may ask for: files
  ```

And what did **not** go away, to be straight about it: the two lists of program names in the shell (`ls`, `cat`, `wc` read; `write`, `rm`, … change). In entry #34 I wrote that this step would "let grant by program name go". It has not. What changed is what the lists *select*: before, a channel to copy; now, a namespace to ask for. The decision "a program called `ls` gets the files" is still made from its name, by the shell, and that is still weak — rename a program and it gets something else. Removing the lists needs a place for a program to *say* what it needs, and a user to agree — a manifest, or the Unix habit of asking at first use. The registry is the mechanism that makes that possible; it is not that. I had the two confused when I wrote the roadmap line.

## The design, in one table

| Decision | Chosen | Rejected, and why |
|---|---|---|
| How a program gets services | One channel to the registry, with its own namespace | A startup flag per service — the ABI grows with every service |
| Who makes the namespace | The registry, on the starter's request | The starter filtering lookups itself — every shell would be a registry |
| What may go in it | Only names the asker can find | Anything registered — authority from nowhere |
| Renaming | Allowed: `files = files.look` | Subsets only — the program would have to know, and choose, its own restriction |
| What a narrowed name stores | The service it means | A reference to the parent's name — breaks when the parent goes |
| End of a namespace | The last holder of its channel going | An explicit "forget it" — which a crashed program never sends |
| A program with no services | No channel at all | An empty namespace — a channel to the registry it has no use for |
| The shell's name lists | Kept, for now | Dropping them with nothing in their place |

## The bugs

None at run time, for the fourth entry running, and by now I think I understand why rather than suspect it. This entry added **one** function to the table (`narrow`), with seven host tests, to a design whose hard decisions — a namespace per channel, the lifetime tied to the handle, "not found" for everything outside — were taken in the last one. The first boot ran 91 programs through narrowed namespaces. When the right structure is there, the next feature is a small one.

What the tools caught:

1. **A lint that taught me a method.** I split the names into pairs with `chunks_exact(2)` and checked `remainder()`. Clippy suggested `as_chunks::<2>()`, which returns the pairs as *arrays of two* plus the leftover slice — so the pattern `let (pairs, []) = ... else { return None }` says "and there must be nothing left over" in the destructuring itself, and each pair is `&[new, old]` with no indexing that could be wrong.

## How I checked it

**On the host**: 7 new tests in `registry` (22 in the crate).

- `protocol` (2): a NARROW encoded and decoded back, with and without names; malformed ones refused — a bad label, a bad name, a name with no partner.
- `table` (5): a narrowed namespace has fewer names and may rename them, and the parent is unchanged; **nobody can give what they cannot find** — a registered service outside the asker's namespace, a nonexistent name, and narrowing a *narrowed* namespace to get back what was taken out; a name may be given before its service registers; narrowed namespaces are never replaced by label, and a program calling itself `shell` does not become init's shell; whatever is refused hands its channel back, including at the table's limit.

**In the boot test** (BIOS and UEFI):

- **Every test that runs a file program now goes through this**: each `ls`, `cat`, `write`, `rm`, `mv`, `cp`, `wc` in the long test — 91 narrowings in one boot — asks the registry for `"files"`. All the earlier expectations (files written, read back, refused when read-only) pass unchanged, which is the real test: the read-only refusals of entry #21 now come from what a name means.
- New expectations in the log: `shell gave ls: files=files.look`, `shell gave write: files`, `shell gave given: files`, `shell gave given: files=files.look`, and `given: it may ask for: files`.
- After `registry crash` (from entry #34's test), `cat welcome` works: the new registry narrows too.
- The count of processes, threads and **channels** is the same at the start and end of the test. Every one of those 91 namespaces was forgotten when its program ended; a leak of one channel would fail this.

`make test64` passed.

**Not tested:**

- I have not typed any of this into a `make run64` window myself.
- **A program running across a registry crash.** Its namespace is lost with the registry; a lookup after that should fail as "the registry cannot be reached", and what it already holds should keep working. No program I have looks anything up late, so nothing exercises it.
- **The table full.** 30 namespaces, two of them init's, so 28 programs with services at once. With background jobs that can now be reached; the shell should say `it cannot be given its services` and not start the program. Host-tested in the table only.
- **A program narrowing its own namespace** for a child. Host-tested; no program starts children.
- **Renaming to anything but `files`.**

## What is still missing

- **The lists of program names in the shell** — see above. Next would be a way for a program to declare what it needs.
- **`with` gives only the files.** `with SERVICE COMMAND` for any name the shell has is now a few lines, and I have not written them.
- **Standard output is still a startup handle**, not a name. I think that is right — it is per run, not a service — but the display *is* a service, and a program that wanted the screen itself (to clear it, to draw) should ask for `"display"`.
- **The registry still has no watchdog**, and now every program start waits on it.
- **A registry crash loses the narrowed namespaces.** init can repeat what it said; nobody can repeat what the shell said. A registry that persisted them — or a shell that re-made them — is possible and not done.
- **One round trip more per program start** (the narrowing), and one per program (the lookup). Not measured; not noticeable.

## What I learned

- **Give a program a name, not a thing — and decide separately what the name means.** That one indirection is where policy lives.
- **Attenuation needs one rule.** "Only from what you have" is the whole security argument, and it fits in a `find`.
- **Renaming is not a convenience.** It is what lets the restricted program be the *same* program.
- **Record what a name means, not where you got it.** Chains of delegation break; a resolved value does not.
- **Lifetimes should follow handles.** I have now used "it ends when the channel closes" four times and never regretted it.
- **Say what a step did not do.** I promised the name lists would go; they did not, and the roadmap line was wrong, not the code.
- In Rust: `as_chunks::<N>()` and array patterns (`&[new, old]`); `flat_map` to turn pairs into a flat sequence; matching a slice against `[]` to mean "empty" in a `match` on the names themselves; a `let ... else` that both destructures and validates.

---

## Testing note

```
make run64
```

Watch the terminal `make run64` was started from: the `[registry]` lines appear there, not in the QEMU window.

**Each program gets its own names**

| Type | You should see |
|---|---|
| `ls` | The listing; in the terminal `[registry] shell gave ls: files=files.look` |
| `write notes hello` | In the terminal `[registry] shell gave write: files` |
| `cat notes` | `hello`; in the terminal `[registry] shell gave cat: files=files.look` |
| `given` | `given: the screen, no files` — and **no** `[registry]` line: with nothing to give, it gets no channel at all |
| `with files given` | `given: it may ask for: files`, then `given: the screen, and the files (N of them), to read and to change`; in the terminal `shell gave given: files` |
| `with readonly given` | `given: it may ask for: files`, then `… to read only`; in the terminal `shell gave given: files=files.look` |

**The same program, the same name, a different meaning**

| Type | You should see |
|---|---|
| `write notes one` then `cat notes` | `one` |
| `with readonly write notes two` | `write: notes: the files are read-only here` and `write exited with code 1` |
| `cat notes` | Still `one` |

**Programs in other places, and odd names** *(not in the automated test)*

| Type | You should see |
|---|---|
| `mkdir tools` then `cp given tools/given` then `with files tools/given` | In the terminal `shell gave given: files` — the label is the last part of the path |

**After the registry is replaced**

| Type | You should see |
|---|---|
| `registry crash` then `ls` | The listing, and a new `shell gave ls: files=files.look` after the registry's `ready` line |

**Nothing left behind**

| Type | You should see |
|---|---|
| `ps`, then run `ls` ten times, then `ps` | The same last line both times — `N processes, N threads, N channels`. Each `ls` made a namespace and a channel; each was forgotten when `ls` ended |

**Many at once** *(not tested at all)*

The registry holds 30 namespaces, two of them init's. I have no tidy way to keep 28 programs with services running at once from the keyboard - the shell's own limit on background jobs comes first. If you ever see `run: NAME: it cannot be given its services: no such service`, that is the table full; `jobs` and `kill N` clear it.

**From the host** — nothing new: the files on the disk are checked as before.

Automated: `make test64`.
