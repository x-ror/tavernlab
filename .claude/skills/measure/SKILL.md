---
name: measure
description: Verify a performance or winrate claim in TavernLab — A/B benchmarks, callgrind, policy/weights/mulligan/tiers runs. Use before claiming a change is faster/slower/stronger, or when updating a number in the README.
---

# Measuring honestly

The README is the claims file: every number in it must be reproducible by a
command on this checkout. Never state a performance or winrate difference
without the matching control.

## Throughput (games/s)

- Never compare two `make bench` runs by eye. A naive best-of-five on a
  shared host reports a ~2% regression for a binary against a copy of itself.
- Use `tools/ab-bench.sh` — read its header first. It interleaves
  A,B,B,A / B,A,A,B and reports mean ratio ± SE. **Run the A/A control
  first** (the same binary on both sides); only trust the A/B result if the
  A/A read as no difference.
- For small differences, prefer `make callgrind`: instruction counts are
  deterministic, so a 0.2% delta is real and needs no control run
  (~10 s under valgrind).

## Strength / policy claims

- `make policy` — greedy vs within-turn search. First column is an A/A seat
  swap and **must read 50.0%**; anything else means the harness is broken and
  the other columns are meaningless.
- `make weights` — one evaluation weight at a time vs its current value.
  First row is the control, must read 50.0%.
- `make mulligan` — first column is the same-policy-different-seeds floor any
  real difference must clear.
- `make tiers` — slow (~200x on the search side); a deliberate run, never
  part of a test loop.

## Reporting

- Report the number measured on *this* build, alongside its control, and note
  the machine noise (the README records 15–22k games/s across sessions on
  identical code).
- If a change moves a README number, update the README (Ukrainian prose) in
  the same change — a stale claim is a bug.
- `bench` throughput dropping after new card implementations is expected, not
  a regression: more real effects means more work per game.
