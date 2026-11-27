#!/bin/sh
# merge-rehearsal.sh [QUEUE] [SCRATCH] - the night's merges, rehearsed by
# day in a sandbox, so that a merge problem is fixed before the night
# (journal entry 139).
#
# Copies TheTaaJ's and TheKernel's main and every branch of QUEUE (default
# ~/.cache/taaj-night/queue) into bare repositories in SCRATCH (default
# ~/.cache/taaj-rehearsal) - nothing on the server is read after the
# fetch, or written - and runs the night runner there, with a stand-in for
# make test64 and --merge: every branch in queue order merged in main,
# tested (the stand-in), merged with tools/merge-item.py, and the next
# stacked on the result. A stand-in test may merge only into such a
# sandbox (night-test.py checks that origin is a local directory).
#
# Then it checks main in the sandbox: a README row for each branch, no
# unnumbered entry left, no "journal: entry" second commit, and
# tools/lanes-check.py. Exits 0 if every branch merged.
#
# It found, the first time (by hand, the same steps): a numbering
# conflict between stacked branches, an old K?-item numbered as new, a
# kernel pointer moved by mistake, two branches with two entries.
set -u
CI=$(cd "$(dirname "$0")/.." && pwd)
QUEUE=${1:-$HOME/.cache/taaj-night/queue}
S=${2:-$HOME/.cache/taaj-rehearsal}
BRANCHES=$(sed 's/#.*//' "$QUEUE" | awk 'NF {print $1}')

rm -rf "$S" && mkdir -p "$S" || exit 1
git -C "$CI" fetch -q origin && git -C "$CI/kernel" fetch -q origin || exit 1
cd "$S" || exit 1
git clone -q --bare "$(git -C "$CI" rev-parse --git-common-dir)" taaj.git
KGIT=$(cd "$CI/kernel" && cd "$(git rev-parse --git-dir)" && pwd)
git clone -q --bare "$KGIT" kernel.git
BGIT=$(cd "$CI/bootloader" && cd "$(git rev-parse --git-dir)" && pwd)
git clone -q --bare "$BGIT" bootloader.git  # read only: no item here changes it
# ...but an item may point at a TheBootloader commit that is only on its
# branch (MaK-WSL's, entry NNN): its branches come too.
git -C "$CI/bootloader" fetch -q origin
for b in $BRANCHES; do
    c=$(git -C "$CI/bootloader" rev-parse -q --verify "origin/$b") && git -C bootloader.git fetch -q "$BGIT" "$c" && git -C bootloader.git update-ref "refs/heads/$b" "$c"
done
for repo in taaj.git kernel.git; do
    git -C $repo for-each-ref --format='%(refname)' refs/heads refs/remotes refs/tags | while read -r r; do git -C $repo update-ref -d "$r"; done
done
for b in main $BRANCHES; do
    c=$(git -C "$CI" rev-parse -q --verify "origin/$b") || { echo "merge-rehearsal: $b is not on the server" >&2; exit 1; }
    git -C taaj.git update-ref "refs/heads/$b" "$c"
    k=$(git -C "$CI/kernel" rev-parse -q --verify "origin/$b") && git -C kernel.git update-ref "refs/heads/$b" "$k"
done
MAIN_BEFORE=$(git -C taaj.git rev-parse main)

git clone -q -b main "$S/taaj.git" work && cd work || exit 1
git config user.name The-Jat
git config user.email "$(git -C "$CI" config user.email)"
git config protocol.file.allow always
git config submodule.kernel.url "$S/kernel.git"
git config submodule.bootloader.url "$S/bootloader.git"
git config merge.rows.name "work-list rows, one item at a time"
git config merge.rows.driver 'if [ -f tools/merge-rows.py ]; then python3 tools/merge-rows.py %O %A %B %P; else git merge-file %A %O %B; fi'
git -c protocol.file.allow=always submodule update -q --init kernel bootloader
git -C kernel config user.name The-Jat
git -C kernel config user.email "$(git -C "$CI" config user.email)"
git -C kernel config protocol.file.allow always
grep -v '^#' "$QUEUE" > "$S/queue"
# REHEARSAL_SETUP_ONLY=1: the sandbox only, to try a tool in (restack.py).
[ -n "${REHEARSAL_SETUP_ONLY:-}" ] && { echo "merge-rehearsal: the sandbox is ready in $S/work"; exit 0; }

# Local submodule clones are refused unless allowed, and a repository's own
# setting does not reach a new clone: allowed for this run only.
GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=protocol.file.allow GIT_CONFIG_VALUE_0=always \
NIGHT_TEST=true NIGHT_OUT="$S/out" TAAJ_SESSION=rehearsal python3 "$CI/tools/night-test.py" --merge "$S/queue" \
    > "$S/runner.log" 2>&1
sed -n '/^| /p' "$S/runner.log" | grep -v '^| Branch\|^|---' | sed 's/`[^`]*\.log`//'

git fetch -q origin
merged=$(git log --format=%s "$MAIN_BEFORE..origin/main" | grep -c ', entry [0-9]*: the journal number given at merge')
wanted=$(echo $BRANCHES | wc -w)
second=$(git log --format=%s "$MAIN_BEFORE..origin/main" | grep -c '^journal: entry')
unnumbered=$(git ls-tree --name-only origin/main docs/journal/ | grep -c '/NNN-')
git checkout -q --detach origin/main && git submodule update -q kernel
lanes=$(python3 tools/lanes-check.py | tail -1)
echo "merge-rehearsal: $merged of $wanted merged; second commits $second; unnumbered entries on main $unnumbered; $lanes"
echo "merge-rehearsal: the sandbox is $S (runner.log, out/reports)"
[ "$merged" = "$wanted" ] && [ "$second" = 0 ] && [ "$unnumbered" = 0 ] && case "$lanes" in *"every open item"*) exit 0 ;; esac
exit 1
