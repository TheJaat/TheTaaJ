#!/bin/sh
# new-session.sh - a working tree of its own for one session (rule 13)
#
#   sh tools/new-session.sh LANE
#
# LANE is one of the lanes of docs/lanes.md (ipc, memory, sched, ...).
# Run it from this machine's first TheTaaJ checkout, the one that has
# built the i686 cross compiler (toolchain/i686). It makes TheTaaJ-LANE
# next to that checkout:
#
#   - a git worktree of TheTaaJ at origin/main, detached: the session
#     makes each item's branch with `git switch -c NAME`;
#   - its submodules (kernel, bootloader) at the commits main points to;
#   - toolchain/i686 linked to this checkout's: it is only read.
#
# If TheTaaJ-LANE is already there, it is left as it is: a session keeps
# one tree for all the items of its lane. Its build/ starts empty, so the
# first `make test64` there builds the ports (LLVM, Clang, lld, libc++)
# once: about 40 minutes, so start it early and work while it runs.

set -eu

LANE=${1:-}
if [ -z "$LANE" ] || [ "$LANE" = "-h" ] || [ "$LANE" = "--help" ]; then
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
fi

ROOT=$(git rev-parse --show-toplevel)
cd "$ROOT"

if ! grep -q "^### \`$LANE\`" docs/lanes.md; then
    echo "new-session: no lane '$LANE' in docs/lanes.md; the lanes are:" >&2
    sed -n 's/^### `\([a-z0-9-]*\)`.*/  \1/p' docs/lanes.md >&2
    exit 1
fi

# The tree goes beside the checkout this is run from, named after the
# checkout's own directory: TheTaaJ -> TheTaaJ-ipc.
TREE="$(dirname "$ROOT")/$(basename "$ROOT")-$LANE"

if [ -e "$TREE" ]; then
    echo "new-session: $TREE is already there; using it as it is"
else
    git fetch --quiet origin
    git worktree add --detach "$TREE" origin/main
    git -C "$TREE" submodule update --init
fi

# The cross compiler is big (1.4 GB) and built once per machine; every
# tree reads the same one.
if [ -d "$ROOT/toolchain/i686" ] && [ ! -e "$TREE/toolchain/i686" ]; then
    ln -s "$ROOT/toolchain/i686" "$TREE/toolchain/i686"
    # toolchain's .gitignore says `i686/`, which matches a directory, not
    # a link to one: kept out of `git status` here instead.
    EXCLUDE=$(cd "$TREE/toolchain" && git rev-parse --path-format=absolute --git-path info/exclude)
    mkdir -p "$(dirname "$EXCLUDE")"
    grep -qx "/i686" "$EXCLUDE" 2>/dev/null || echo "/i686" >> "$EXCLUDE"
fi
if [ ! -e "$TREE/toolchain/i686" ]; then
    echo "new-session: no toolchain/i686 here; build it first (tools/setup-wsl.sh step 6," >&2
    echo "  or toolchain/toolchain_i686.sh), then run this again" >&2
    exit 1
fi

echo
echo "Lane '$LANE' has its tree: $TREE"
echo "Next, in that tree:"
echo "  1. read docs/start-here.md, then the lane's section of docs/lanes.md"
echo "  2. take the lane's first open item; make its branch in each repository"
echo "     it changes:  git switch -c NAME   (and git -C kernel switch -c NAME)"
echo "  3. start the first build early:  make iso64 && make test64-shared"
