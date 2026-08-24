#!/usr/bin/env python3
"""Generate a Wild gauntlet from the implemented Wild pool.

**These are baselines, not the meta.** The Standard gauntlet is twelve
real top-legend decks typed in from trackers. There is no equivalent
source for Wild that we are allowed to use: the legal posture rules out
scraping HSReplay or Untapped (design A2 and "Правовий режим"), so a
scraped Wild meta is not on the table.

What this builds instead is one deck per class, assembled from the cards
the simulator actually implements, using the same curve-and-quality seed
the optimizer uses for its own candidates. That gives Wild evaluation a
measurable, reproducible opponent field today. It does **not** claim to
be what people are queuing into.

Replace any of them with the real thing as you get decklists:

    python3 update_meta.py --format wild --add "Even Shaman" "AAEBAa..."

    python3 scripts/make_wild_gauntlet.py          # (re)generate
    python3 scripts/make_wild_gauntlet.py --force  # overwrite existing
"""
import argparse
import json
import os
import random
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import console  # noqa: E402
from hs2 import carddata, decks, formats  # noqa: E402

CLASSES = ("DEATHKNIGHT", "DEMONHUNTER", "DRUID", "HUNTER", "MAGE",
           "PALADIN", "PRIEST", "ROGUE", "SHAMAN", "WARLOCK", "WARRIOR")


def build(seed=11):
    from hs2.optimize import build_seed, deck_counts
    carddata.ensure_defs(formats.WILD)
    rng = random.Random(seed)
    out = {}
    for cls in CLASSES:
        name = f"{cls.title()} Wild Baseline"
        try:
            deck = build_seed(cls, name, rng)
        except Exception as exc:                  # too thin a pool
            print(f"  - {cls}: пропущено ({exc})")
            continue
        cards = sorted(deck_counts(deck).items())
        total = sum(n for _, n in cards)
        if total < 30:
            print(f"  - {cls}: пропущено (лише {total} карт у пулі)")
            continue
        out[name] = {
            "class": cls,
            "cards": [[cn, n] for cn, n in cards],
            "sideboard": [],
            "total": total,
            # Provenance travels with the deck: `load_meta` ignores extra
            # keys, and nobody should mistake this for tracker data.
            "source": "generated from the implemented Wild pool "
                      "(scripts/make_wild_gauntlet.py) - not tracker meta",
        }
        print(f"  + {name}: {total} карт")
    return out


def main():
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--force", action="store_true",
                    help="перезаписати наявний гаунтлет")
    ap.add_argument("--seed", type=int, default=11)
    args = ap.parse_args()

    path = decks.gauntlet_path(formats.WILD)
    if os.path.exists(path) and not args.force:
        raise SystemExit(f"{path} вже існує — використайте --force, "
                         f"якщо справді хочете перезаписати.")
    out = build(args.seed)
    if not out:
        raise SystemExit("Жодного класу не вдалося зібрати.")
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=1)
    print(f"\n{len(out)} колод -> {path}")
    print("Це baseline, не мета. Замінюйте справжніми колодами:")
    print('  python3 update_meta.py --format wild --add "Назва" "AAEBA..."')


if __name__ == "__main__":
    console.init()
    main()
