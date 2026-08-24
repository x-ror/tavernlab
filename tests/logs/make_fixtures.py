#!/usr/bin/env python3
"""Dev tool: turn a real Power.log session into committable fixtures.

Policy (design §4.4): `tests/logs/raw/` is gitignored and holds full logs
with real battletags; `tests/logs/fixtures/` holds stripped, gzipped slices.

    python3 tests/logs/make_fixtures.py <Power.log> [--out tests/logs/fixtures]

Redacts battletags, Blizzard account ids, and the Zone.log companion's
account blocks. Everything else is left byte-identical so the fixture keeps
exercising the real parser edge cases (broken option nesting, orphaned
BLOCK_END, dormant tag caching).
"""
import argparse
import gzip
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(
    os.path.dirname(os.path.abspath(__file__)))))

_BATTLETAG = re.compile(r"\b[^\s\[\]=]{2,24}#\d{3,6}\b")
_ACCOUNT = re.compile(r"(GameAccountId|AccountId)=\[hi=\d+ lo=\d+\]")


def redact(lines):
    """Battletag -> PlayerN#0000N, stable within one slice."""
    names, out = {}, []
    for ln in lines:
        for m in _BATTLETAG.finditer(ln):
            tag = m.group(0)
            if tag not in names:
                n = len(names) + 1
                names[tag] = f"Player{n}#{n:05d}"
        out.append(ln)
    subbed = []
    for ln in out:
        for real, fake in names.items():
            if real in ln:
                ln = ln.replace(real, fake)
        subbed.append(_ACCOUNT.sub(r"\1=[hi=0 lo=0]", ln))
    return subbed, names


def main():
    from capture import hslog_import as imp
    ap = argparse.ArgumentParser()
    ap.add_argument("power_log")
    ap.add_argument("--out", default=os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "fixtures"))
    ap.add_argument("--prefix", default="real")
    ap.add_argument("--pick", default="",
                    help="comma-separated slice indexes, in file order; "
                         "default: the 2 smallest that have a winner")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    games = list(imp.parsed_games(args.power_log))
    if args.pick:
        chosen = [games[int(x)] for x in args.pick.split(",")]
    else:
        # A fixture without a winner cannot pin the result field, so prefer
        # finished games; the smallest keep the suite fast.
        chosen = sorted([g for g in games if g[2].winner_pid is not None],
                        key=lambda g: len(g[0]))[:2]
    for i, (raw, events, summ, _parser) in enumerate(chosen):
        lines, names = redact(raw)
        path = os.path.join(args.out, f"{args.prefix}_game{i + 1}.log.gz")
        with gzip.open(path, "wt", encoding="utf-8") as fh:
            fh.writelines(lines)
        print(f"{path}  {len(lines)} lines, {len(events)} events, "
              f"winner_pid={summ.winner_pid} turns={summ.turns} "
              f"classes={summ.classes} redacted={len(names)}")


if __name__ == "__main__":
    main()
