#!/bin/sh
# part-time.sh TARGET -c COMMAND - the shell make runs a test64 part's
# recipe lines with (tools/test64.sh sets it, journal entry NNN): runs
# the line as /bin/sh would, and writes down when it started and ended,
#
#   TARGET START END
#
# in seconds, to $TAAJ_PART_TIMES. A part's time is from its first line's
# start to its last line's end; test64.sh adds them up and orders the next
# run's parts by them, the longest first. Its prerequisites' lines come
# here too, under their own names. Without TAAJ_PART_TIMES, only /bin/sh.
target=$1
shift
[ -n "${TAAJ_PART_TIMES:-}" ] || exec /bin/sh "$@"
start=$(date +%s.%N)
/bin/sh "$@"
status=$?
echo "$target $start $(date +%s.%N)" >> "$TAAJ_PART_TIMES"
exit $status
