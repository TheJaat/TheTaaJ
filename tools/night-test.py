#!/usr/bin/env python3
"""night-test.py [QUEUE] - tests the queued branches, one after another,
and writes a report for the morning (journal entry 135).
night-test.py --all - our own CI (J93): main, and every branch of ours
not merged yet whose newest commit has not been tested (entry 136).

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

A branch that fails stops the queue there. With --wait MINUTES, it waits
for the fix (entry 136): the session fixes the failure in its own tree
(its quick checks and make test64-again take the test64 lock while the
runner waits) and pushes; within a minute the runner tests the branch
again, and goes on to the next only once it passed - the branches after
it are stacked on its result, so testing them on a failed one would be
done twice. No new commit for MINUTES: it stops, and the branches after
it are "not run". Without --wait, the first failure ends the stack.

--merge: each branch that passed is merged, then the next is tested on
main - the user's standing yes (rule 2, 2026-11-27): an item that passes
its whole make test64, on exactly what is merged, may be merged without
asking. Before merging, if main moved since the test: main's new commits
touch this item's files, or kernel/abi - test again (rule 15); else
tools/merge-main.py merges it in. Then tools/merge-item.py, with the
entry's title and TAAJ_SESSION (say TheJat-WSL/ci) as the README row. A
branch with more than one unnumbered entry, or one merge-item refuses,
is left for its session, and the branches stacked on it are not run.
(entry 139)

A part that fails is run once more (make test64-again, only the parts
that failed) before the branch is called failed: a boot that missed its
timing window - k114-desktop-bench's power64-test, on night 2 - passes
the second time, and a real failure fails twice. Passed on that retry,
the report says which parts needed it (flaky: J95's list). A failed
shared build is not retried: it fails the same way. (entry 149)

--hunt ROUNDS: after the queue, the race hunt (J95): main, as the
queue left it, in the night tree; its shared builds; then ROUNDS rounds
of every part (`ALL=1 tools/race-hunt.sh ROUNDS`), each round under the
test64 lock. The hunt goes on from night to night where it stopped
(build/race-hunt/summary.txt), and the report has the night's rounds
and the tally of all of them: which parts ever failed, how often. A
failed round's files are kept in build/race-hunt/round-N/. (entry NNN)

--from BRANCH: starts the queue at BRANCH - after a runner fault, the
rest of the night's queue. The branches before it are passed over; one
already on main counts as merged, for the branches stacked on it.
(entry 149)

NIGHT_TEST="make unsafe-check" (say) runs that instead of make test64, to
try this script itself in minutes - and then nothing is pushed or
posted: only a real make test64 makes a branch's merge fit to push.
NIGHT_HUNT stands in for the race hunt (and with NIGHT_TEST, the shared
builds before it are not built). NIGHT_TEST_AGAIN, with it, stands in for make test64-again (the retry
is not tried without it).
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
MERGE = False  # --merge: the user's standing yes (rule 2)
FROM = None  # --from BRANCH: the queue from that branch on
HUNT = 0  # --hunt ROUNDS: the race hunt on main, after the queue (J95)
ARGS = []
_args = iter(sys.argv[1:])
for _a in _args:
    if _a == "--wait":
        WAIT = int(next(_args))
    elif _a == "--merge":
        MERGE = True
    elif _a == "--from":
        FROM = next(_args)
    elif _a == "--hunt":
        HUNT = int(next(_args))
    elif _a != "--all":
        ARGS.append(_a)
QUEUE = ARGS[0] if ARGS else os.path.join(HOME, ".cache/taaj-night/queue")
OUT = os.environ.get("NIGHT_OUT", os.path.join(HOME, ".cache/taaj-night"))  # a rehearsal's own
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


SESSIONS = {}  # branch -> the session whose item it is (`session=` on its line)
OWN = os.environ.get("TAAJ_SESSION", "")


def queue():
    """[(branch, the branch it is stacked on, or None)]. A line is
    `BRANCH [after BELOW] [session=MACHINE/LANE]`: the session whose item
    it is, when not this one's (entry 141) - the README row names it, and
    a test that fails is that session's to fix: the runner does not wait
    for it, and goes on."""
    entries = []
    for line in open(QUEUE):
        words = line.split("#", 1)[0].split()
        if not words:
            continue
        for word in words[1:]:
            if word.startswith("session="):
                SESSIONS[words[0]] = word[len("session="):]
        words = [w for w in words if not w.startswith("session=")]
        after = words[2] if len(words) == 3 and words[1] == "after" else None
        entries.append((words[0], after))
    return entries


def theirs(branch):
    """An item of another session's, handed to this queue (user's choice B,
    2026-11-27): tested and merged here, but fixed by its own session."""
    return bool(SESSIONS.get(branch)) and SESSIONS[branch] != OWN


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
    # TheKernel and TheBootloader in this tree are clones of their own:
    # without the author, a merge commit there fails ("empty ident name"),
    # on a machine with no global git user (found rehearsing MaK-WSL's
    # branches, entry 141). The author of the repository's own config.
    for sub in (KERNEL, os.path.join(TREE, "bootloader")):
        if os.path.isdir(sub):
            for key in ("user.name", "user.email"):
                value = git("config", key, cwd=SOURCE, check=False)
                if value and not git("config", key, cwd=sub, check=False):
                    git("config", key, value, cwd=sub)
    # toolchain's .gitignore says `i686/`, which matches a directory, not
    # the link to one: kept out of `git status` (as new-session.sh does),
    # or merge-item.py finds the tree not clean and merges nothing - the
    # first real night's first merge (entry 139).
    if os.path.islink(os.path.join(TREE, "toolchain/i686")):
        exclude = git("rev-parse", "--path-format=absolute", "--git-path", "info/exclude", cwd=os.path.join(TREE, "toolchain"))
        os.makedirs(os.path.dirname(exclude), exist_ok=True)
        if "/i686" not in open(exclude).read().split() if os.path.exists(exclude) else True:
            with open(exclude, "a") as out:
                out.write("/i686\n")


def prepare(branch, base_taaj, base_kernel):
    """Checks the branch out with its base merged in. Returns the TheKernel
    commit it uses, or raises with why it could not."""
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    git("reset", "-q", "--hard")
    # A merge that failed before may have left TheKernel or TheBootloader
    # half merged: the next checkout would refuse (entry 141).
    for sub in (KERNEL, os.path.join(TREE, "bootloader")):
        if os.path.isdir(sub):
            run("git", "merge", "--abort", cwd=sub, check=False)
            run("git", "reset", "-q", "--hard", cwd=sub, check=False)
    git("checkout", "-q", "--detach", f"origin/{branch}")
    git("submodule", "update", "-q", "--init", "kernel", "bootloader")
    # main as it is: its own pointer, nothing merged - but TheBootloader
    # built from nothing all the same, if its commit is another (below).
    kernel_branch = git("ls-remote", "origin", f"refs/heads/{branch}", cwd=KERNEL) if branch != "main" else ""
    kernel = None
    if kernel_branch:
        git("checkout", "-q", "--detach", f"origin/{branch}", cwd=KERNEL)
        if run("git", "merge", "-q", "--no-edit", base_kernel, cwd=KERNEL, check=False).returncode != 0:
            git("merge", "--abort", cwd=KERNEL, check=False)
            raise RuntimeError(f"TheKernel's {branch} does not merge with {base_kernel[:10]}")
        kernel = git("rev-parse", "HEAD", cwd=KERNEL)
    merged = run("git", "merge", "-q", "--no-edit", base_taaj, check=False) if branch != "main" else run("true", check=False)
    if merged.returncode != 0:
        conflicts = git("diff", "--name-only", "--diff-filter=U").split()
        if [c for c in conflicts if c != "kernel"] and os.path.exists(os.path.join(TREE, "tools/merge-main.py")):
            # Bookkeeping conflicts - lines main has since numbered, rows
            # the work list's split moved - settled as a session would
            # (tools/merge-main.py; the tree is detached, so TheKernel is
            # left to the lines above).
            git("merge", "--abort", check=False)
            run("python3", "tools/merge-main.py", base_taaj, check=False)
            conflicts = git("diff", "--name-only", "--diff-filter=U").split()
        if [c for c in conflicts if c != "kernel"]:
            git("merge", "--abort", check=False)
            raise RuntimeError(f"does not merge with {base_taaj[:10]}: {' '.join(conflicts)}")
    if not kernel and merged.returncode != 0:
        # No TheKernel branch of its own, and the base moved the pointer:
        # the base's.
        kernel = git("ls-tree", base_taaj, "kernel").split()[2]
    if kernel:
        git("checkout", "-q", kernel, cwd=KERNEL)
        git("add", "kernel")
    merging = run("git", "rev-parse", "-q", "--verify", "MERGE_HEAD", check=False).returncode == 0
    if git("diff", "--cached", "--name-only") or merging:
        git("commit", "-q", "--no-edit", "-m", f"Merge {base_taaj[:10]} into {branch} (night test)")
    # The merge may have moved `kernel` (the base's newer one): check out
    # what the result says, not what the branch had - or the programs are
    # built against an older ABI than the kernel's.
    if not kernel_branch:
        git("submodule", "update", "-q", "kernel")
    git("submodule", "update", "-q", "bootloader")
    # TheBootloader's Makefile does not know which .inc files an assembly
    # file includes: built before at another commit, its objects can be
    # stale - k130-any-size's `global BootHeader`, added to an .inc, was
    # missing from the stage2.o the branch before had built, and the link
    # failed (entry 148). It builds in seconds: from nothing, whenever its
    # commit is not the one built last.
    boot = os.path.join(TREE, "bootloader")
    built = os.path.join(TREE, "build", ".bootloader-commit")
    commit = git("rev-parse", "HEAD", cwd=boot)
    if not os.path.exists(built) or open(built).read().strip() != commit:
        subprocess.run(["rm", "-rf", os.path.join(boot, "build")])
        os.makedirs(os.path.dirname(built), exist_ok=True)
        open(built, "w").write(commit + "\n")
    return kernel or git("ls-tree", "HEAD", "kernel").split()[2]


TOTALS = {"tests": 0, "testing": 0, "cpu": 0.0, "waiting": 0, "passed": 0, "failed": 0, "fixes": 0}


def test(log, again=False):
    """make test64 (or the stand-in), under the machine's test64 lock.
    Every line of the log is stamped with the minutes and seconds since
    the start - a part's output comes as a block when it ends
    (--output-sync), so its stamp is when it finished - and the end
    has the CPU time and memory /usr/bin/time measured; all for the
    morning's look at what to make faster (entry 136). Returns (status,
    seconds, the parts that failed, CPU seconds). again: make test64-again,
    the parts build/test64-failed has."""
    command = os.environ.get("NIGHT_TEST_AGAIN" if again else "NIGHT_TEST", f"make --no-print-directory -k test64{'-again' if again else ''}")
    setup = f". {LLVM} && make_flags=\"LLVM_TBLGEN=$LLVM_TBLGEN\" || make_flags=" if os.path.isfile(LLVM) else "make_flags="
    measured = "/usr/bin/time -f 'night-test: CPU %U s user, %S s system; at most %M KB; %e s' " if os.path.exists("/usr/bin/time") else ""
    script = f"{setup}; flock /tmp/taaj-test64.lock sh -c \"echo night-test: the test64 lock taken; {measured}{command} $make_flags\""
    failed_list = os.path.join(TREE, "build/test64-failed")
    if os.path.exists(failed_list) and not again:
        os.remove(failed_list)
    start = time.time()
    with open(log, "w") as out:
        child = subprocess.Popen(["sh", "-c", script], cwd=TREE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                 text=True, errors="replace")
        for line in child.stdout:
            gone = int(time.time() - start)
            out.write(f"[{gone // 60:02}:{gone % 60:02}] {line}")
        status = child.wait()
    seconds = int(time.time() - start)
    text = open(log, errors="replace").read()
    # make test64 writes what failed (tools/test64.sh); else the FAIL lines.
    if os.path.exists(failed_list) and open(failed_list).read().split():
        failed = open(failed_list).read().split()
    else:
        failed = sorted({m for m in re.findall(r"FAIL[^:]*: (.{0,80})", text)})
    cpu = re.search(r"night-test: CPU ([\d.]+) s user, ([\d.]+) s system", text)
    cpu = float(cpu.group(1)) + float(cpu.group(2)) if cpu else 0.0
    TOTALS["tests"] += 1
    TOTALS["testing"] += seconds
    TOTALS["cpu"] += cpu
    TOTALS["passed" if status == 0 else "failed"] += 1
    return status, seconds, failed, cpu


def test_retrying(log):
    """test(), and the parts that failed once more (make test64-again)
    before the branch is called failed. Returns test()'s four and a note
    for the report: which parts passed only on the retry."""
    status, seconds, failed, cpu = test(log)
    if status == 0 or (STAND_IN and not os.environ.get("NIGHT_TEST_AGAIN")) or not failed or {"test64-shared", "test64-parts"} & set(failed):
        return status, seconds, failed, cpu, ""
    print(f"night-test: failed: {' '.join(failed)} - once more (make test64-again)", flush=True)
    status, more, again, more_cpu = test(log.removesuffix(".log") + "-retry.log", again=True)
    if status == 0:
        note = f" on a retry of {', '.join(failed)} (failed once: flaky?)"
    else:
        note = " (failed on the retry too)"
        failed = again or failed
    return status, seconds + more, failed, cpu + more_cpu, note


def minutes(seconds):
    return f"{seconds // 60} min {seconds % 60:02} s" if seconds >= 60 else f"{seconds} s"


def fixes(before, after):
    """The commits that came between a failed try and the next: the fix,
    as its commit messages say."""
    found = []
    for cwd, commits in ((TREE, (before[0], after[0])), (KERNEL, (before[1], after[1]))):
        if commits[0] and commits[1] and commits[0] != commits[1]:
            out = git("log", "--format=%h %s", f"{commits[0]}..{commits[1]}", cwd=cwd, check=False)
            found += [line[:120] for line in out.splitlines()]
    return found


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
        status, seconds, failed, cpu, retried = test_retrying(log)
        if status == 0:
            said_result = f"passed in {seconds // 60} min, {how}{retried}"
        else:
            said_result = f"failed: {'; '.join(failed[:3]) or 'see the night report'}"
        for where, commit in ((taaj_repository, taaj.get(branch)), (kernel_repository, kernel_head)):
            post(where, commit, "success" if status == 0 else "failure", said_result)
        remember(branch)
        lines.append(f"| {branch} | main | **{'passed' if status == 0 else f'FAILED ({status})'}** | {minutes(seconds)} (CPU {minutes(int(cpu))}) | {'; '.join(failed[:5])} | `{log}` |")
        open(report, "w").write("\n".join(lines) + "\n")


def night_queue(logs, report, lines):
    """The queue, one branch after another. A branch that fails is fixed
    before the next is tested (with --wait): the runner waits for a new
    commit on it, tests it again, and goes on only once it passed - the
    branches after it are stacked on its result, and a test on a failed
    base would have to be run again after the fix anyway."""
    tested = {}  # branch -> (TheTaaJ commit, TheKernel commit), if it passed
    starting = FROM is None
    for branch, after in queue():
        if not starting:
            if branch != FROM:
                # Passed over (--from); on main already - merge-item.py's
                # commit there, the branch itself deleted - merged.
                on_main = bool(git("log", "-1", "--format=%h", "--grep", f"^{re.escape(branch)}, entry [0-9]*:", "origin/main")) \
                    or run("git", "merge-base", "--is-ancestor", f"origin/{branch}", "origin/main", check=False).returncode == 0
                if on_main:
                    tested[branch] = ("origin/main", "origin/main")
                lines.append(f"| {branch} | | passed over (--from {FROM}){': on main already' if on_main else ''} | | | |")
                continue
            starting = True
        if after:
            if after not in tested:
                lines.append(f"| {branch} | after {after} | **not run**: {after} did not pass | | | |")
                continue
            (base_taaj, base_kernel) = tested[after]
            said = f"main, {after} merged" if base_taaj == "origin/main" else f"{after}'s result"
        else:
            base_taaj, base_kernel, said = "origin/main", "origin/main", "main"
        attempt = 0
        while True:
            git("fetch", "-q", "origin")
            git("fetch", "-q", "origin", cwd=KERNEL)
            now = heads_of(branch)
            if not now[0]:
                lines.append(f"| {branch} | {said} | **not pushed**: no such branch on the server | | | |")
                break
            attempt += 1
            again = f" (try {attempt})" if attempt > 1 else ""
            print(f"night-test: {branch} on {said}{again}", flush=True)
            log = os.path.join(logs, f"{branch}-{attempt}.log")
            try:
                main_tested = git("rev-parse", "origin/main")
                kernel = prepare(branch, base_taaj, base_kernel)
                status, seconds, failed, cpu, retried = test_retrying(log)
                why = "; ".join(failed[:5]) or "see the log"
            except RuntimeError as error:
                status, seconds, why, log, cpu, retried = "does not merge", 0, str(error), None, 0.0, ""
            took = f"{minutes(seconds)} (CPU {minutes(int(cpu))})" if seconds else ""
            if status == 0:
                tested[branch] = (git("rev-parse", "HEAD"), kernel)
                note = push(branch)
                lines.append(f"| {branch} | {said}{again} | **passed**{retried}{note} | {took} | | `{log}` |")
                write(report, lines)
                if MERGE and (not STAND_IN or rehearsal()) and fine_base(after, tested):
                    merged, why = merge_now(branch, main_tested)
                    if merged == "retest":
                        lines.append(f"| {branch} | | main moved meanwhile and touches this item ({why}): tested again (rule 15) | | | |")
                        write(report, lines)
                        base_taaj, base_kernel, said = "origin/main", "origin/main", "main (moved)"
                        continue
                    lines.append(f"| {branch} | | {'**merged**: ' if merged else '**not merged**: '}{why} | | | |")
                    write(report, lines)
                    if merged:
                        # The next branch stacks on main, which has this one now.
                        tested[branch] = ("origin/main", "origin/main")
                    else:
                        tested.pop(branch, None)
                break
            result = "**does not merge**" if status == "does not merge" else f"**FAILED** ({status})"
            lines.append(f"| {branch} | {said}{again} | {result}{retried} | {took} | {why} | {f'`{log}`' if log else ''} |")
            write(report, lines)
            waited = time.time()
            if theirs(branch):
                lines.append(f"| {branch} | | failed - **for {SESSIONS[branch]} to fix**; not waited for: the branches stacked on it are not run | | | |")
                write(report, lines)
                break
            if not wait_for_fix(branch, now):
                TOTALS["waiting"] += int(time.time() - waited)
                stop = f"no fix pushed for {WAIT} min" if WAIT else "failed, no --wait"
                lines.append(f"| {branch} | | {stop}: the branches after it are not run | | | |")
                break
            git("fetch", "-q", "origin")
            git("fetch", "-q", "origin", cwd=KERNEL)
            fixed_by = fixes(now, heads_of(branch))
            TOTALS["waiting"] += int(time.time() - waited)
            TOTALS["fixes"] += 1
            lines.append(f"| {branch} | | fixed in {minutes(int(time.time() - waited))} by: {'; '.join(fixed_by) or 'a new commit'} | | | |")
            write(report, lines)


def hunt(lines):
    """--hunt: ROUNDS rounds of every part on main (tools/race-hunt.sh)."""
    lines += ["", f"## The race hunt (J95): {HUNT} rounds on main", ""]
    try:
        prepare("main", "origin/main", "origin/main")
    except RuntimeError as error:
        lines.append(f"- not run: main could not be checked out: {error}")
        return
    if not os.path.exists(os.path.join(TREE, "tools/race-hunt.sh")):
        lines.append("- not run: main has no tools/race-hunt.sh")
        return
    setup = f". {LLVM} && make_flags=\"LLVM_TBLGEN=$LLVM_TBLGEN\" || make_flags=" if os.path.isfile(LLVM) else "make_flags="
    start = time.time()
    shared = "true" if STAND_IN else "flock /tmp/taaj-test64.lock make --no-print-directory test64-shared $make_flags"
    built = subprocess.run(["sh", "-c", f"{setup}; {shared}"],
                           cwd=TREE, capture_output=True, text=True)
    if built.returncode != 0:
        lines.append(f"- not run: main's shared builds failed: {built.stdout[-300:]}{built.stderr[-300:]}")
        return
    summary = os.path.join(TREE, "build/race-hunt/summary.txt")
    before = open(summary).read() if os.path.exists(summary) else ""
    command = os.environ.get("NIGHT_HUNT", f"ALL=1 tools/race-hunt.sh {HUNT}")
    done = subprocess.run(["sh", "-c", f"{setup}; {command}"], cwd=TREE, capture_output=True, text=True)
    after = open(summary).read() if os.path.exists(summary) else ""
    took = int(time.time() - start)
    TOTALS["testing"] += took
    tonight = [l for l in after[len(before):].splitlines() if l.startswith("round ") or l.startswith("    ")]
    lines.append(f"- {minutes(took)}; tonight's rounds:")
    lines += [f"    {l}" for l in tonight] or ["    (none)"]
    lines += ["", "Every round so far (build/race-hunt/tally.txt in the night tree):", ""]
    lines += [f"    {l}" for l in done.stdout.strip().splitlines()[-30:]]


def rehearsal():
    """A stand-in test may merge only into a sandbox: origin a directory on
    this machine (tools/merge-rehearsal.sh), never the server."""
    url = git("remote", "get-url", "origin")
    return url.startswith("/") or url.startswith("file:")


def fine_base(after, tested):
    """A branch is merged only if what it stands on is on main already."""
    return not after or tested.get(after) == ("origin/main", "origin/main")


def merge_now(branch, main_tested):
    """--merge: the passed branch merged, as its session would (the user's
    standing yes, rule 2): main merged in again if it moved - tested again
    first if main's new commits touch this item's files or kernel/abi (rule
    15) - then tools/merge-item.py, with the entry's title as the README
    row. Returns (True, what) merged, (False, why) not, ("retest", why)."""
    git("fetch", "-q", "origin")
    git("fetch", "-q", "origin", cwd=KERNEL)
    main_now = git("rev-parse", "origin/main")
    if main_now != main_tested:
        theirs = set(git("diff", "--name-only", main_tested, main_now).split())
        ours = set(git("diff", "--name-only", main_tested, "HEAD").split())
        touched = sorted((theirs & ours) - {"kernel"})
        if "kernel" in theirs:
            old_k = git("ls-tree", main_tested, "kernel").split()[2]
            new_k = git("ls-tree", main_now, "kernel").split()[2]
            if git("diff", "--name-only", old_k, new_k, "--", "abi", cwd=KERNEL, check=False):
                touched.append("kernel/abi")
        if touched:
            return "retest", ", ".join(touched[:5])
    entries = [f for f in os.listdir(os.path.join(TREE, "docs/journal")) if f.startswith("NN" + "N-")]
    if len(entries) != 1:
        return False, f"want one unnumbered entry, found {len(entries)}: merge it by hand"
    title = open(os.path.join(TREE, "docs/journal", entries[0])).readline().lstrip("# ").strip()
    session = SESSIONS.get(branch) or os.environ.get("TAAJ_SESSION", f"{MACHINE}/ci")
    row = f"{title} ({session}, merged after its night test)"
    kernel_branch = bool(git("ls-remote", "origin", f"refs/heads/{branch}", cwd=KERNEL))
    head = git("rev-parse", "HEAD")
    try:
        # A git error here - the branch checked out in another tree, say -
        # leaves this branch for its session; it does not end the night
        # (the first real night's last merge, entry 140).
        git("switch", "-q", "-C", branch, head)
        if kernel_branch:
            git("switch", "-q", "-C", branch, git("rev-parse", "HEAD", cwd=KERNEL), cwd=KERNEL)
        if main_now != main_tested:
            done = run("python3", "tools/merge-main.py", check=False)
            if done.returncode != 0:
                return False, "main moved and does not merge in by itself: " + done.stdout.strip()[-200:]
            if run("git", "rev-parse", "-q", "--verify", "MERGE_HEAD", check=False).returncode == 0:
                git("commit", "-q", "--no-edit")
            if kernel_branch:
                run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", cwd=KERNEL, check=False)
            run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", check=False)
        done = run("python3", "tools/merge-item.py", row, check=False)
        said = (done.stdout + done.stderr).strip().splitlines()
        if done.returncode != 0:
            # What it said before its last line is the why (lanes-check's
            # findings, say): kept, on one line.
            return False, " / ".join(l.strip() for l in said[-6:] if l.strip())[:400] or "merge-item failed"
        return True, (said[-1] if said else "merged")[:200]
    except RuntimeError as why:
        return False, str(why)[:200]
    finally:
        run("git", "reset", "-q", "--hard", check=False)
        run("git", "switch", "-q", "--detach", check=False)
        run("git", "merge", "--abort", cwd=KERNEL, check=False)
        run("git", "reset", "-q", "--hard", cwd=KERNEL, check=False)
        run("git", "switch", "-q", "--detach", cwd=KERNEL, check=False)


def heads_of(branch):
    """(TheTaaJ commit, TheKernel commit) of a branch on the server."""
    return (git("ls-remote", "origin", f"refs/heads/{branch}").split("\t")[0] or None,
            git("ls-remote", "origin", f"refs/heads/{branch}", cwd=KERNEL).split("\t")[0] or None)


def wait_for_fix(branch, before):
    """With --wait: True once the branch has a new commit (the fix), False
    after WAIT minutes without one."""
    if not WAIT:
        return False
    print(f"night-test: {branch} failed - waiting for a fix (up to {WAIT} min)", flush=True)
    deadline = time.time() + WAIT * 60
    while time.time() < deadline:
        time.sleep(60)
        if heads_of(branch) != before:
            return True
    return False


def push(branch):
    """A passed branch's merges, pushed - TheKernel's first - so that the
    morning merge finds the base already merged in. Not after a stand-in."""
    if STAND_IN:
        return ""
    pushed = []
    if git("ls-remote", "origin", f"refs/heads/{branch}", cwd=KERNEL):
        pushed.append(run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", cwd=KERNEL, check=False).returncode)
    pushed.append(run("git", "push", "-q", "origin", f"HEAD:refs/heads/{branch}", check=False).returncode)
    return "" if not any(pushed) else " (the merge could not be pushed: the branch moved)"


def write(report, lines):
    open(report, "w").write("\n".join(lines) + "\n")


STARTED = time.time()


def checked_out_elsewhere(branches):
    """{branch: tree} for queued branches checked out in a tree other than
    the night's: git will not check one branch out twice, so the night
    could test it but not merge it."""
    found, tree_path = {}, None
    for line in git("worktree", "list", "--porcelain", cwd=SOURCE).splitlines():
        if line.startswith("worktree "):
            tree_path = line[9:]
        elif line.startswith("branch refs/heads/"):
            name = line[18:]
            if name in branches and os.path.realpath(tree_path) != os.path.realpath(TREE):
                found[name] = tree_path
    return found


def main():
    if FROM and FROM not in [b for b, _ in queue()]:
        sys.exit(f"night-test: --from {FROM}, but it is not in the queue {QUEUE}")
    if MERGE and not ALL:
        busy = checked_out_elsewhere({b for b, _ in queue()})
        if busy:
            sys.exit("night-test: --merge, but these queued branches are checked out in other trees "
                     "(switch those trees to another branch, or `git switch --detach`, first):\n"
                     + "\n".join(f"  {b}: {t}" for b, t in sorted(busy.items())))
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
        if HUNT:
            hunt(lines)
            write(report, lines)
    else:
        every_branch(logs, report, lines, taaj, kernel_heads, taaj_repository, kernel_repository, record)
    wall = int(time.time() - STARTED)
    lines += ["", "## For the morning", "",
              f"- Wall time {minutes(wall)}: testing {minutes(TOTALS['testing'])} ({TOTALS['tests']} runs: "
              f"{TOTALS['passed']} passed, {TOTALS['failed']} failed), waiting for fixes {minutes(TOTALS['waiting'])} "
              f"({TOTALS['fixes']} fixes), the rest fetching and merging.",
              f"- CPU {minutes(int(TOTALS['cpu']))} in all"
              + (f", {TOTALS['cpu'] / max(TOTALS['testing'], 1):.1f} CPUs busy on average while testing." if TOTALS['cpu'] else "."),
              "- Each log's lines are stamped with the time since its test started: a part's block ends when the part did - "
              "the slow ones are where to look first.",
              "- The session's notes, what it fixed and what could be faster: the night's journal entry."]
    open(report, "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nnight-test: the report is {report}")


main()
