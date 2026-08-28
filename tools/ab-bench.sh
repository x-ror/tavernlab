#!/bin/bash
# Compare two `tavernsim` binaries' throughput, honestly.
#
# The naive way to do this -- run A five times, run B five times, compare the
# best of each -- does not work on a shared host. Two things go wrong, and
# both were caught by measuring a binary against a byte-identical copy of
# itself:
#
#   * the host drifts. Between two runs a few minutes apart the same binary
#     measured 16 852 and 17 491 games/s, a 4% move with no code involved;
#   * position matters. In an A,B,B,A round the outer slots run consistently
#     faster, and a plain ABBA average reports the B binary about 2% slow --
#     for a copy of A.
#
# So each round here runs A,B,B,A and then B,A,A,B: every binary gets both
# outer and both inner slots, which cancels the position bias, and the two
# binaries always run within seconds of each other, which cancels the drift.
# The result is a per-round ratio; report the mean of the ratios and the
# standard error, not the best run.
#
# Always run the A/A control before believing an A/B result:
#
#   cp path/to/main/tavernsim /tmp/copy
#   tools/ab-bench.sh path/to/main/tavernsim /tmp/copy 8 /tmp/control.txt
#
# Whatever that prints is this harness's floor on this host. A difference
# smaller than the floor is not a difference.
#
# usage: tools/ab-bench.sh <binary A> <binary B> [rounds] [output file]
set -u
A=${1:?binary A}; B=${2:?binary B}; N=${3:-8}; OUT=${4:-/dev/stdout}
run() { "$1" bench 2>&1 | grep -o "^ *1 thread.*" | grep -o "[0-9]* games/s" | grep -o "^[0-9]*"; }
tmp=$(mktemp)
for _ in $(seq 1 "$N"); do
  a1=$(run "$A"); b1=$(run "$B"); b2=$(run "$B"); a2=$(run "$A")
  b3=$(run "$B"); a3=$(run "$A"); a4=$(run "$A"); b4=$(run "$B")
  echo "$a1 $a2 $a3 $a4 $b1 $b2 $b3 $b4" >> "$tmp"
done
cp "$tmp" "$OUT" 2>/dev/null || true
python3 - "$tmp" <<'PY'
import statistics as st, sys
rows = [[int(x) for x in line.split()] for line in open(sys.argv[1])]
ratios = [st.mean(r[4:8]) / st.mean(r[0:4]) for r in rows]
se = st.stdev(ratios) / len(ratios) ** 0.5 if len(ratios) > 1 else float("nan")
print(f"A median {int(st.median([x for r in rows for x in r[0:4]]))} games/s")
print(f"B median {int(st.median([x for r in rows for x in r[4:8]]))} games/s")
print(f"B/A: {(st.mean(ratios) - 1) * 100:+.2f}% +/- {se * 100:.2f}% "
      f"(mean of {len(ratios)} paired rounds, {len(rows) * 8} runs)")
PY
rm -f "$tmp"
