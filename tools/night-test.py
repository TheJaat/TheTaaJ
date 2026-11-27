#!/usr/bin/env python3
"""night-test.py [QUEUE] - tests the queued branches, one after another,
and writes a report for the morning (journal entry NNN).
night-test.py --all - our own CI (J93): main, and every branch of ours
not merged yet whose newest commit has not been tested (entry NNN).

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

--all needs no queue. It finds the branches itself: every branch on
TheTaaJ's server (`origin`; only the project's own branches - never a
pull request or a fork, rule 14) that main does not already contain,
`llvm-23` left out, and main itself, tested as it is. A branch is
tested again only when it (or its TheKernel branch) has a new commit:
what was tested is kept in ~/.cache/taaj-night/tested. Nothing is
pushed - the branches are other sessions'. Each result is posted
instead, as a commit status on the branch's newest commit, TheTaaJ's
and TheKernel's (`gh`; context "MACHINE test64", MACHINE from
TAAJ_MACHINE, say TheJat-WSL), where the session whose branch it is
sees it on GitHub.

A branch that fails does not stop the ones stacked on it: they are
tested on its result all the same - their own failures show the same
night - and marked "on a failed base"; nothing on a failed base is
pushed. With --wait MINUTES, it does not end there: it waits for a fix
(entry NNN). Every two minutes it looks at the server, and a failed
branch with a new commit is tested again, and every branch stacked on
it after it - until all passed, or no new commit came for MINUTES.
Meanwhile the session fixes the failure in its own tree (its quick
checks and make test64-again wait for the test64 lock between two
branches) and pushes.

NIGHT_TEST="make unsafe-check" (say) runs that instead of make test64, to
try this script itself in minutes - and then nothing is pushed or
posted: only a real make test64 makes a branch's merge fit to push.
"""
import datetime
import json
import os
import re
import subprocess
import sys
import time

HOME = os.path.expanduser("~")
ALL = "--all" in sys.argv[1:]
WAIT = 0  # --wait MINUTES
ARGS = []
_args = iter(sys.argv[1:])
for _a in _args:
    if _a == "--wait":
        WAIT = int(next(_args))
    elif _a != "--all":
        ARGS.append(_a)
QUEUE = ARGS[0] if ARGS else os.path.join(HOME, ".cache/taaj-night/queue")
OUT = os.path.join(HOME, ".cache/taaj-night")
TESTED = os.path.join(OUT, "tested")
MACHINE = os.environ.get("TAAJ_MACHINE", "ci")
STAND_IN = bool(os.environ.get("NIGHT_TEST"))
NOT_TESTED = {"main", "llvm-23"}  # main is tested as it is, first
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


def heads(cwd):
    """Branch -> newest commit, on the server."""
    found = {}
    for line in git("ls-remote", "--heads", "origin", cwd=cwd).splitlines():
        commit, ref = line.split()
        found[ref.removeprefix("refs/heads/")] = commit
    return found


def tested_before():
    try:
        return json.load(open(TESTED))
    except (OSError, ValueError):
        return {}


def everything(taaj, kernel):
    """--all: main, then each branch main does not contain, whose commits
    (TheTaaJ's and TheKernel's) were not tested before. Branches only from
    our own server: no pull request, no fork (rule 14)."""
    before = tested_before()
    entries = []
    if before.get("main") != [taaj["main"], kernel.get("main")]:
        entries.append(("main", None))
    for branch in sorted(taaj):
        if branch in NOT_TESTED:
            continue
        if run("git", "merge-base", "--is-ancestor", taaj[branch], "origin/main", check=False).returncode == 0:
            continue  # merged already
        if before.get(branch) != [taaj[branch], kernel.get(branch)]:
            entries.append((branch, None))
    return entries


def post(repository, commit, state, description):
    """A commit status on GitHub, beside the branch; a failure to post is
    only reported."""
    if STAND_IN or not commit:
        return
    done = subprocess.run(["gh", "api", "-X", "POST", f"repos/{repository}/statuses/{commit}",
                           "-f", f"state={state}", "-f", f"context={MACHINE} test64",
                           "-f", f"description={description[:140]}"],
                          capture_output=True, text=True)
    if done.returncode != 0:
        print(f"night-test: could not post to {repository}: {done.stderr.strip()[:200]}", flush=True)


def repository(cwd):
    """owner/name of the server's repository, from origin's address."""
    url = git("remote", "get-url", "origin", cwd=cwd)
    return re.search(r"[:/]([^/:]+/[^/]+?)(\.git)?$", url).group(1)


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
    if branch == "main":
        # main as it is: its own pointer, nothing merged.
        return git("ls-tree", "HEAD", "kernel").split()[2]
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


def every_branch(logs, report, lines, taaj, kernel_heads, taaj_repository, kernel_repository, record):
    """--all: each branch once, on main; results posted, nothing pushed."""
    def remember(branch):
        if not STAND_IN:  # tested again only when the branch moves
            record[branch] = [taaj.get(branch), kernel_heads.get(branch)]
            with open(TESTED, "w") as out:
                json.dump(record, out, indent=1, sort_keys=True)

    for branch, _ in everything(taaj, kernel_heads):
        log = os.path.join(logs, f"{branch}.log")
        print(f"night-test: {branch} on main", flush=True)
        try:
            kernel = prepare(branch, "origin/main", "origin/main")
        except RuntimeError as why:
            post(taaj_repository, taaj.get(branch), "failure", f"does not merge with main: {why}")
            remember(branch)
            lines.append(f"| {branch} | main | **does not merge** | | {why} | |")
            continue
        # main's own pointer for main (TheKernel's main may be newer).
        kernel_head = kernel if branch == "main" else kernel_heads.get(branch)
        how = "main as it is" if branch == "main" else "with main merged in"
        for where, commit in ((taaj_repository, taaj.get(branch)), (kernel_repository, kernel_head)):
            post(where, commit, "pending", f"make test64 running, {how}")
        status, seconds, failed = test(log)
        if status == 0:
            said_result = f"passed in {seconds // 60} min, {how}"
        else:
            said_result = f"failed: {'; '.join(failed[:3]) or 'see the night report'}"
        for where, commit in ((taaj_repository, taaj.get(branch)), (kernel_repository, kernel_head)):
            post(where, commit, "success" if status == 0 else "failure", said_result)
        remember(branch)
        lines.append(f"| {branch} | main | **{'passed' if status == 0 else f'FAILED ({status})'}** | {seconds // 60} min | {'; '.join(failed[:5])} | `{log}` |")
        open(report, "w").write("\n".join(lines) + "\n")


def night_queue(logs, report, lines):
    """The queue, in rounds: the first tests every branch; each later one
    the failed branches that have a new commit, and what is stacked on
    them. With --wait, it waits for such commits."""
    results = {}  # branch -> {"fine", "heads", "result"}
    entries = queue()
    idle_since = time.time()
    round_number = 0
    while True:
        git("fetch", "-q", "origin")
        git("fetch", "-q", "origin", cwd=KERNEL)
        taaj, kernel_heads = heads(TREE), heads(KERNEL)
        changed = set()
        for branch, after in entries:
            now = (taaj.get(branch), kernel_heads.get(branch))
            done = results.get(branch)
            moved = done is not None and not done["fine"] and done["heads"] != now
            if done is not None and not moved and after not in changed:
                continue
            log = os.path.join(logs, f"{branch}-{round_number}.log")
            if after:
                base = results.get(after)
                if not base or not base["result"]:
                    lines.append(f"| {branch} | after {after} | **not run**: {after} has no result to stack on | | | |")
                    results[branch] = {"fine": False, "heads": now, "result": None}
                    continue
                base_taaj, base_kernel = base["result"]
                base_fine = base["fine"]
                said = f"{after}'s result" + ("" if base_fine else " - **a failed base**")
            else:
                base_taaj, base_kernel, said, base_fine = "origin/main", "origin/main", "main", True
            if not now[0]:
                lines.append(f"| {branch} | {said} | **not pushed**: no such branch on the server | | | |")
                results[branch] = {"fine": False, "heads": now, "result": None}
                continue
            if round_number:
                said += f" (round {round_number + 1})"
            print(f"night-test: {branch} on {said}", flush=True)
            changed.add(branch)
            try:
                kernel = prepare(branch, base_taaj, base_kernel)
            except RuntimeError as why:
                lines.append(f"| {branch} | {said} | **does not merge** | | {why} | |")
                results[branch] = {"fine": False, "heads": now, "result": None}
                open(report, "w").write("\n".join(lines) + "\n")
                continue
            status, seconds, failed = test(log)
            fine = status == 0 and base_fine
            # The result, for the branches stacked on this one - passed or not.
            results[branch] = {"fine": fine, "heads": now, "result": (git("rev-parse", "HEAD"), kernel)}
            if fine:
                # Pushed, so the morning merge finds the base already merged in.
                pushed = []
                if STAND_IN:
                    pass  # a stand-in test: nothing is pushed
                else:
                    if kernel_heads.get(branch):
                        pushed.append(run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", cwd=KERNEL, check=False).returncode)
                    pushed.append(run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", check=False).returncode)
                note = "" if not any(pushed) else " (the merge could not be pushed: the branch moved)"
                lines.append(f"| {branch} | {said} | **passed**{note} | {seconds // 60} min | | `{log}` |")
            elif status == 0:
                lines.append(f"| {branch} | {said} | passed, **on a failed base**: not pushed | {seconds // 60} min | | `{log}` |")
            else:
                lines.append(f"| {branch} | {said} | **FAILED** ({status}) | {seconds // 60} min | {'; '.join(failed[:5]) or 'see the log'} | `{log}` |")
            open(report, "w").write("\n".join(lines) + "\n")
        round_number += 1
        if changed:
            idle_since = time.time()
        if all(r["fine"] for r in results.values()):
            return
        if time.time() - idle_since >= WAIT * 60:
            if WAIT:
                lines.append(f"| | | no fix pushed for {WAIT} min: stopped | | | |")
            return
        if changed:
            waiting = [b for b, r in results.items() if not r["fine"]]
            print(f"night-test: waiting for a fix to {', '.join(waiting)} (up to {WAIT} min)", flush=True)
        time.sleep(120)


def main():
    tree()
    night = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    logs = os.path.join(OUT, "logs", night)
    os.makedirs(logs, exist_ok=True)
    report = os.path.join(OUT, "reports", f"{night}.md")
    os.makedirs(os.path.dirname(report), exist_ok=True)
    lines = [f"# {'CI' if ALL else 'Night test'} {night}", "", "| Branch | Base | Result | Time | What failed | Log |", "|---|---|---|---|---|---|"]
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    taaj, kernel_heads = heads(TREE), heads(KERNEL)
    taaj_repository, kernel_repository = repository(TREE), repository(KERNEL)
    record = tested_before()
    if not ALL:
        night_queue(logs, report, lines)
    else:
        every_branch(logs, report, lines, taaj, kernel_heads, taaj_repository, kernel_repository, record)
    open(report, "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nnight-test: the report is {report}")


main()
