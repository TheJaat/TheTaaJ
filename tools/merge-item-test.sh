#!/bin/sh
# merge-item-test.sh - tests tools/merge-item.py without touching the
# server (journal entry NNN).
#
#   sh tools/merge-item-test.sh SCRATCH
#
# Makes local bare copies of TheTaaJ and TheKernel at today's mains (from
# this tree's objects), a tree cloned from them, and an item branch in
# both (a new entry, an `NNN` comment in TheKernel, a new K item). Then four
# runs, each on a fresh copy: a clean merge; TheKernel's push refused;
# TheTaaJ's push refused (a pre-push hook pushes a rival commit to main
# first); and the run after that, with main merged in. SCRATCH is any
# directory to work in; nothing outside it is written.
set -u
S=$(cd "$1" && pwd)
CI=$(cd "$(dirname "$0")/.." && pwd)

# The marker, in two pieces: merge-item.py numbers it on the lines a
# branch adds, and this file's own lines must keep it when it is merged.
M=NN; M="${M}N"; export M

setup() (
set -eu
TM=$(git -C "$CI" rev-parse origin/main)
KM=$(git -C "$CI/kernel" rev-parse origin/main)
KGIT=$(cd "$CI/kernel" && cd "$(git rev-parse --git-dir)" && pwd)
rm -rf "$S/mt"; mkdir -p "$S/mt"; cd "$S/mt"
git clone -q --bare "$(git -C "$CI" rev-parse --git-common-dir)" taaj.git
git -C taaj.git update-ref refs/heads/main "$TM"
git clone -q --bare "$KGIT" kernel.git
git -C kernel.git update-ref refs/heads/main "$KM"
git clone -q -b main "$S/mt/taaj.git" work
cd work
git config user.name The-Jat; git config user.email t@example.invalid
git config submodule.kernel.url "$S/mt/kernel.git"
git -c protocol.file.allow=always submodule update -q --init kernel
git -C kernel config user.name The-Jat; git -C kernel config user.email t@example.invalid
git -C kernel checkout -q -B mt-test
echo "# a test comment (journal entry $M)" >> kernel/kernel64/scripts/fuzz-test.sh
git -C kernel commit -qam "test change, entry $M"
git -C kernel push -q origin mt-test
git switch -q -c mt-test
cp "$CI/tools/merge-item.py" tools/  # the version being tested
printf '# A test entry\n\n*Journal entry #%s · a test. It keeps `%s-name.md` as it is.*\n\nSee K?-mt.\n\n## Testing note\n\nNothing.\n' "$M" "$M" > "docs/journal/$M-mt.md"
python3 - <<'PY'
p='docs/who-does-what.md'; s=open(p).read()
i=s.index('| K189 |'); j=s.index('\n',i)
s=s[:j+1]+"| K?-mt | **A test item** | S | for merge-item's test (entry "+__import__("os").environ["M"]+") |\n"+s[j+1:]
open(p,'w').write(s)
p='docs/lanes.md'; s=open(p).read()
o="- E6-E8, J50, J97, which come later or wait for the user.\n"; assert o in s; s=s.replace(o,o+"- K?-mt, a test.\n"); open(p,'w').write(s)
PY
git add -A kernel docs tools
git commit -qm "test item, entry $M"
git push -q origin mt-test
echo "set up in $S/mt/work: TheTaaJ $(git rev-parse --short HEAD), TheKernel $(git -C kernel rev-parse --short HEAD)"
)

rival_hook() { # rival_hook GITDIR BARE: a hook that makes BARE's main move once
    cat > "$1/hooks/pre-push" <<HOOK
#!/bin/sh
[ -e "$1/rival-done" ] && exit 0
touch "$1/rival-done"
grep -q "refs/heads/main" || exit 0
d=\$(mktemp -d); git clone -q "$2" "\$d/r" && cd "\$d/r" && git -c user.name=Rival -c user.email=r@x.invalid commit -q --allow-empty -m "a rival merge" && git push -q origin HEAD:main
exit 0
HOOK
    chmod +x "$1/hooks/pre-push"
}
fresh() { (setup) > /dev/null && cd "$S/mt/work" && git config protocol.file.allow always && git -C kernel config protocol.file.allow always \
    && git -C kernel fetch -q origin && git -C kernel merge -q --no-edit origin/main && git -C kernel push -q origin mt-test \
    && git add kernel && { git diff --cached --quiet || git commit -qm "test item: TheKernel main merged in"; } && git push -q origin mt-test; }
row="A test row: the merge script's own test"

echo "== A: clean"
fresh || exit 1
python3 tools/merge-item.py "$row"; echo "exit $?"
T=$S/mt/taaj.git; K=$S/mt/kernel.git
echo "items: $(git -C $T show main:docs/who-does-what.md | grep -cE '^\| K[0-9]+ \| \*\*A test item')  rules untouched: $(git -C $T show main:docs/who-does-what.md | grep -c 'K?-hubs')"
echo "pointer $(git -C $T ls-tree main kernel | awk '{print substr($3,1,7)}') = kernel main $(git -C $K rev-parse --short main)"

echo "== B: TheKernel's push refused"
fresh || exit 1
before=$(git -C $S/mt/taaj.git rev-parse main); kb=$(git -C kernel rev-parse HEAD)
rival_hook "$(git -C kernel rev-parse --absolute-git-dir)" "$S/mt/kernel.git"
python3 tools/merge-item.py "$row"; echo "exit $?"
echo "TheTaaJ main unchanged: $([ "$(git -C $S/mt/taaj.git rev-parse main)" = "$before" ] && echo yes || echo NO)"
echo "TheKernel main is the rival's: $(git -C $S/mt/kernel.git log -1 --format=%s main)"
echo "kernel branch back as it was: $([ "$(git -C kernel rev-parse HEAD)" = "$kb" ] && echo yes || echo NO) on $(git -C kernel branch --show-current)"
echo "entry still unnumbered: $(ls docs/journal | grep -c '^NNN-mt.md')  tree clean: $(git status --porcelain | grep -vc bootloader)"

echo "== C: TheTaaJ's push refused"
fresh || exit 1
tb=$(git rev-parse HEAD)
rival_hook "$(git rev-parse --absolute-git-dir)" "$S/mt/taaj.git"
python3 tools/merge-item.py "$row"; echo "exit $?"
echo "TheTaaJ main is the rival's: $(git -C $S/mt/taaj.git log -1 --format=%s main)"
echo "TheKernel main has the item: $(git -C $S/mt/kernel.git log -1 --format=%s main)"
echo "branch back as it was: $([ "$(git rev-parse HEAD)" = "$tb" ] && echo yes || echo NO) on $(git branch --show-current)  entry still unnumbered: $(ls docs/journal | grep -c '^NNN-mt.md')  clean: $(git status --porcelain | grep -vc bootloader)"

echo "== D: after C, TheTaaJ's main merged in, and run again"
rm -f "$(git rev-parse --absolute-git-dir)/hooks/pre-push"
git fetch -q origin && git merge -q --no-edit origin/main && git push -q origin mt-test
kc=$(git -C kernel rev-parse HEAD)
python3 tools/merge-item.py "$row"; echo "exit $?"
echo "pointer $(git -C $S/mt/taaj.git ls-tree main kernel | awk '{print substr($3,1,7)}') = the item's TheKernel commit $(echo $kc | cut -c1-7)"
git -C $S/mt/taaj.git show main:docs/journal/README.md | grep "^| 124 |" | grep -o "| written | .*"
echo "items: $(git -C $S/mt/taaj.git show main:docs/who-does-what.md | grep -cE '^\| K[0-9]+ \| \*\*A test item')"
