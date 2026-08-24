#!/usr/bin/env python3
"""Search for two-card swaps that only pay off together.

`hs2/optimize.py` climbs one card at a time and keeps a swap when it
measures more than +1.5%. That shape cannot find a combo: if each half is
individually a downgrade, every single step is rejected and the pair is
never reached. This script goes looking for exactly those.

For every candidate pair it measures four gauntlet runs:

    d_pair  both swaps applied
    d_a     only the first
    d_b     only the second
    synergy = d_pair - (d_a + d_b)

A pair is reported as a **combo** when the pair helps, neither half would
have survived the hill-climber's threshold, and the synergy is several
times the measurement noise. Anything weaker is printed as a near miss
rather than dressed up as a finding.

The pool is whatever format the deck's own deckstring names, so this runs
on Standard and Wild alike.

    python3 scripts/find_combos.py "AAECA..."
    python3 scripts/find_combos.py --deck-file my.txt --pairs 40 --games 400
"""
import argparse
import itertools
import os
import random
import statistics
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import console  # noqa: E402
from hs2 import carddata, decks, formats  # noqa: E402

# Affinity vocabulary: two cards are worth trying together when they talk
# about the same thing. Random pairs from a 250-card pool would be ~30k
# combinations, nearly all of them meaningless.
TEXT_TOKENS = (
    "Corpse", "Herald", "Dragon", "Undead", "Beast", "Murloc", "Demon",
    "Elemental", "Mech", "Pirate", "Naga", "Quilboar", "Draenei",
    "Deathrattle", "Battlecry", "Spell Damage", "Secret", "Weapon",
    "Overload", "Discover", "Combo", "Outcast", "Imbue", "Dark Gift",
    "Taunt", "Rush", "Divine Shield", "Lifesteal", "Freeze", "Silence",
    "costs", "Draw", "Armor", "Reborn", "Titan", "Excavate", "Starship",
)


def tokens(d):
    """What this card is 'about', as a set of tags."""
    out = set(d.races or [])
    if d.school:
        out.add(d.school)
    text = (d.text or "").lower()
    for kw in TEXT_TOKENS:
        if kw.lower() in text:
            out.add(kw)
    return out


def affine_pairs(pool, rng, want, seen):
    """Candidate (in_a, in_b) pairs that share at least one tag."""
    by_tag = {}
    for d in pool:
        for t in tokens(d):
            by_tag.setdefault(t, []).append(d)
    tags = [t for t, ds in by_tag.items() if 2 <= len(ds) <= 60]
    rng.shuffle(tags)
    out = []
    for tag in itertools.cycle(tags or [None]):
        if tag is None or len(out) >= want:
            break
        ds = by_tag[tag]
        a, b = rng.sample(ds, 2)
        key = tuple(sorted((a.name, b.name)))
        if key in seen:
            continue
        seen.add(key)
        out.append((a, b, tag))
        if len(out) >= want:
            break
    return out


def main():
    ap = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("deckstring", nargs="?")
    ap.add_argument("--deck-file")
    ap.add_argument("--pairs", type=int, default=25,
                    help="скільки пар перевірити (типово 25)")
    ap.add_argument("--games", type=int, default=400,
                    help="боїв на суперника за оцінку (типово 400)")
    ap.add_argument("--confirm", type=int, default=8,
                    help="у скільки разів більша вибірка для перевірки "
                         "кандидатів (типово 8; 1 = не перевіряти)")
    ap.add_argument("--procs", type=int, default=14)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()

    from evaluate import try_resolve
    from hs2.optimize import class_pool, deck_counts, swap
    from hs2.sim import gauntlet_winrate

    code = args.deckstring
    if args.deck_file:
        with open(args.deck_file, encoding="utf-8-sig") as fh:
            code = fh.read().strip()
    if not code:
        ap.error("передайте деккод або --deck-file")

    deck, info = try_resolve(code)
    if deck is None:
        raise SystemExit(
            "Колода не резолвиться: "
            + (", ".join(info.get("unimplemented")
                         or info.get("illegal") or [])
               or str(info.get("error"))))
    fmt = info.get("format") or formats.STANDARD
    gauntlet = decks.load_meta(fmt)
    if not gauntlet:
        raise SystemExit(f"Немає гаунтлета для формату «{fmt}»")

    def wr(d):
        avg, _ = gauntlet_winrate(d, gauntlet, args.games,
                                  processes=args.procs, fmt=fmt)
        return avg

    print(f"Колода: {info['cls']} [{fmt}], {info['total']} карт")
    print(f"Гаунтлет: {len(gauntlet)} колод, "
          f"{args.games * len(gauntlet)} боїв на оцінку")

    base = wr(deck)
    print(f"База: {base:.3f}\n")

    counts = deck_counts(deck)
    pool = [d for d in class_pool(deck.cls) if d.name not in counts]
    rng = random.Random(args.seed)
    pairs = affine_pairs(pool, rng, args.pairs, set())
    # Cut from the weakest third, not at random: pulling two good cards
    # buries a real synergy under the loss of what it replaced.
    from hs2.optimize import card_quality
    outs = [n for n, _ in sorted(
        ((n, card_quality(carddata.get_def(n))) for n in counts),
        key=lambda x: x[1])][:max(4, len(counts) // 3)]
    print(f"Пул кандидатів: {len(pool)}; перевіряю {len(pairs)} пар "
          f"за спорідненістю тегів\n")

    # Threshold the hill-climber uses, and the noise scale measured on
    # this repo: SD of a delta is ~0.6% at 1440 games, and scales as
    # 1/sqrt(n).
    KEEP = 0.015
    # Measured on this repo: SD of a *delta* is ~0.6% at 1440 games and
    # scales as 1/sqrt(n). Synergy is not a delta - it is
    # wr(both) - wr(a) - wr(b) + wr(base), four estimates rather than
    # two, so its own SD is about twice the per-estimate one. Testing it
    # against a delta's SD is how you manufacture combos that are noise.
    d_noise = 0.006 * (1440 / (args.games * len(gauntlet))) ** 0.5
    syn_noise = 2 * d_noise / 2 ** 0.5
    print(f"Шум дельти ~{d_noise:.2%}, шум синергії ~{syn_noise:.2%}; "
          f"поріг сходження {KEEP:.1%}")
    print(f"Оголошую комбо лише від {3 * syn_noise:.2%} синергії "
          f"(3σ); {args.pairs} перевірок -> хибних очікую <0.1\n")
    noise = syn_noise

    combos, near = [], []
    t0 = time.time()
    for n, (a, b, tag) in enumerate(pairs, 1):
        o1, o2 = rng.sample(outs, 2)
        try:
            only_a = swap(deck, o1, a.name)
            only_b = swap(deck, o2, b.name)
            both = swap(only_a, o2, b.name)
        except (ValueError, KeyError):
            continue
        d_a, d_b = wr(only_a) - base, wr(only_b) - base
        d_pair = wr(both) - base
        syn = d_pair - (d_a + d_b)
        invisible = d_a < KEEP and d_b < KEEP
        row = (syn, d_pair, d_a, d_b, a.name, b.name, o1, o2, tag)
        if d_pair > KEEP and invisible and syn > 3 * noise:
            combos.append(row)
            mark = "КОМБО"
        elif syn > 3 * noise:
            near.append(row)
            mark = "синергія"
        else:
            mark = ""
        print(f"  [{n:>2}/{len(pairs)}] +{a.name} +{b.name}  ({tag})"
              f"  пара {d_pair:+.3f}  половини {d_a:+.3f}/{d_b:+.3f}"
              f"  синергія {syn:+.3f} {mark}")

    print(f"\nСкринінг: {time.time() - t0:.0f}s\n")

    # Second stage. Picking the best of N noisy measurements overstates it
    # every time - the winner's curse - so nothing screened is believed
    # until it survives a bigger sample. On this repo's own data both
    # first-round "combos" shrank by two thirds and one vanished entirely.
    if combos and args.confirm > 1:
        big = args.games * args.confirm
        cn = 2 * (0.006 * (1440 / (big * len(gauntlet))) ** 0.5) / 2 ** 0.5
        print(f"=== Перевірка {len(combos)} кандидатів на "
              f"{big * len(gauntlet)} боях (шум синергії ~{cn:.2%}) ===")

        def wr_big(d):
            avg, _ = gauntlet_winrate(d, gauntlet, big,
                                      processes=args.procs, fmt=fmt)
            return avg

        base_big = wr_big(deck)
        survived = []
        for _syn, _dp, _da, _db, na, nb, o1, o2, tag in combos:
            only_a = swap(deck, o1, na)
            both = swap(only_a, o2, nb)
            da = wr_big(only_a) - base_big
            db = wr_big(swap(deck, o2, nb)) - base_big
            dp = wr_big(both) - base_big
            syn = dp - (da + db)
            ok = (syn > 3 * cn and dp > KEEP and da < KEEP and db < KEEP)
            print(f"  +{na} +{nb}: пара {dp:+.2%}, половини "
                  f"{da:+.2%}/{db:+.2%}, синергія {syn:+.2%} "
                  f"({syn / cn:.1f}σ) -> "
                  f"{'ПІДТВЕРДЖЕНО' if ok else 'відкинуто'}")
            if ok:
                survived.append((syn, dp, da, db, na, nb, o1, o2, tag))
        combos = survived
        print()

    if combos:
        print("=== КОМБО: пара працює, кожна половина окремо відхилилась "
              "би ===")
        for syn, dp, da, db, na, nb, o1, o2, tag in sorted(combos,
                                                           reverse=True):
            print(f"  -{o1} -{o2}  +{na} +{nb}   [{tag}]")
            print(f"     разом {dp:+.1%}, окремо {da:+.1%} і {db:+.1%}, "
                  f"синергія {syn:+.1%}")
    else:
        print("Комбо не знайдено на цій вибірці пар.")
    if near:
        print("\n=== синергія є, але пара не переступила поріг ===")
        for syn, dp, da, db, na, nb, _o1, _o2, tag in sorted(near,
                                                             reverse=True)[:5]:
            print(f"  +{na} +{nb} [{tag}]: пара {dp:+.1%}, "
                  f"синергія {syn:+.1%}")


if __name__ == "__main__":
    console.init()
    main()
