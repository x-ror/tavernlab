#!/usr/bin/env python3
"""Evaluate YOUR deck against the current top-legend meta gauntlet.

Usage:
    python3 evaluate.py "<deckstring>" [--games N] [--optimize]
    python3 evaluate.py --file mydeck.txt --games 2000

The deck is simulated N games (default 1000) against each of the 11
decoded top-legend tracker decks. Cards whose mechanics are not
implemented in the simulator are reported; the run aborts unless every
card is covered (fidelity-first policy). --optimize additionally
hill-climbs card swaps and prints suggestions with measured win-rate
deltas.
"""
import argparse
import sys
import time

sys.path.insert(0, ".")

from hs2 import carddata, decks
from hs2.deckstring import decode
from hs2.decks import Deck
from hs2.sim import gauntlet_winrate
from hs2.optimize import optimize, deck_counts

HERO_CLASS = {  # hero dbfId prefixes resolved via card data instead
}


def try_resolve(code, name="Your Deck"):
    """Returns (deck_or_None, info: {cls, total, unimplemented, missing})."""
    if not carddata.DEFS:
        carddata.build_defs()
    by_dbf = {d.dbf: d for d in carddata.DEFS.values()}
    try:
        info = decode(code)
    except Exception as e:
        return None, {"error": f"Невалідний деккод: {e}"}
    hero = by_dbf.get(info["heroes"][0]) if info["heroes"] else None
    if hero is not None and hero.cls != "NEUTRAL":
        cls = hero.cls
    else:
        votes = {}
        for dbf, n in info["cards"]:
            d = by_dbf.get(dbf)
            if d is not None and d.cls != "NEUTRAL":
                votes[d.cls] = votes.get(d.cls, 0) + n
        if not votes:
            return None, {"error": "Не вдалося визначити клас колоди"}
        cls = max(votes, key=votes.get)
    cardlist, missing, unimpl = [], [], []
    for dbf, n in info["cards"]:
        d = by_dbf.get(dbf)
        if d is None:
            missing.append(dbf)
            continue
        if not d.implemented:
            unimpl.append(d.name)
        cardlist.append((d.name, n))
    for dbf, n, owner in info["sideboards"]:
        d = by_dbf.get(dbf)
        owner_d = by_dbf.get(owner)
        if d is None:
            missing.append(dbf)
            continue
        if owner_d is not None and owner_d.name == "Commander Beatrix":
            n = 10
        if not d.implemented:
            unimpl.append(d.name)
        cardlist.append((d.name, n))
    meta = {"cls": cls, "total": sum(n for _, n in cardlist),
            "unimplemented": sorted(set(unimpl)), "missing": missing,
            "cards": sorted(cardlist)}
    if missing or unimpl:
        return None, meta
    return Deck.from_names(name, cls, "midrange", sorted(cardlist)), meta


def resolve_deck(code, name="Your Deck"):
    carddata.build_defs()
    by_dbf = {d.dbf: d for d in carddata.DEFS.values()}
    info = decode(code)
    hero = by_dbf.get(info["heroes"][0])
    if hero is not None and hero.cls != "NEUTRAL":
        cls = hero.cls
    else:
        # fall back: majority class among the deck's class cards
        votes = {}
        for dbf, n in info["cards"]:
            d = by_dbf.get(dbf)
            if d is not None and d.cls != "NEUTRAL":
                votes[d.cls] = votes.get(d.cls, 0) + n
        if not votes:
            raise SystemExit("Cannot determine deck class")
        cls = max(votes, key=votes.get)
    cardlist = []
    missing = []
    unimpl = []
    for dbf, n in info["cards"]:
        d = by_dbf.get(dbf)
        if d is None:
            missing.append(dbf)
            continue
        if not d.implemented:
            unimpl.append(d.name)
        cardlist.append((d.name, n))
    for dbf, n, owner in info["sideboards"]:
        d = by_dbf.get(dbf)
        owner_d = by_dbf.get(owner)
        if d is None:
            missing.append(dbf)
            continue
        if owner_d is not None and owner_d.name == "Commander Beatrix":
            n = 10
        if not d.implemented:
            unimpl.append(d.name)
        cardlist.append((d.name, n))
    if missing:
        raise SystemExit(f"Cards not in Standard dataset (dbf): {missing}")
    if unimpl:
        print("ЦІ КАРТИ ЩЕ НЕ РЕАЛІЗОВАНІ В СИМУЛЯТОРІ:")
        for cn in sorted(set(unimpl)):
            print(f"  - {cn}")
        print("Симуляція з ними була б неправильною, тому зупиняюсь.\n"
              "Заміни їх або попроси реалізувати ці карти.")
        raise SystemExit(1)
    total = sum(n for _, n in cardlist)
    print(f"Клас: {cls.title()}, карт у колоді: {total}")
    return Deck.from_names(name, cls, "midrange", sorted(cardlist))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("deckstring", nargs="?")
    ap.add_argument("--file", help="file containing the deckstring")
    ap.add_argument("--games", type=int, default=1000,
                    help="games per meta opponent (default 1000)")
    ap.add_argument("--optimize", action="store_true",
                    help="also search for improving card swaps")
    ap.add_argument("--procs", type=int, default=14)
    args = ap.parse_args()

    code = args.deckstring
    if args.file:
        code = open(args.file).read().strip()
    if not code:
        ap.error("pass a deckstring or --file")

    deck = resolve_deck(code)
    metas = decks.load_meta()
    t0 = time.time()
    avg, rates = gauntlet_winrate(deck, metas, args.games,
                                  processes=args.procs)
    dt = time.time() - t0
    n_total = args.games * len(metas)
    print(f"\n=== {n_total} боїв за {dt:.0f}с ===")
    print(f"Середній winrate проти мети: {avg:.1%}\n")
    for name, r in sorted(rates.items(), key=lambda x: -x[1]):
        bar = "#" * int(r * 30)
        print(f"  {r:6.1%}  {name:28} {bar}")

    if args.optimize:
        print("\n=== Пошук покращень (свопи карт) ===")
        best, wr, hist = optimize(deck, metas, n_eval=max(
            200, args.games // 5), rounds=2, proposals=12,
            processes=args.procs)
        kept = [(o, i, w, d) for o, i, w, d in hist if d > 0.015]
        if kept:
            print(f"\nРекомендовані заміни (нове середнє: {wr:.1%}):")
            for o, i, w, d in kept:
                print(f"  ВИЙМИ «{o}» → ДОДАЙ «{i}»  ({d:+.1%})")
        else:
            print("Явних покращень не знайдено — колода вже щільна.")
        near = sorted([h for h in hist if 0 < h[3] <= 0.015],
                      key=lambda h: -h[3])[:3]
        if near:
            print("Межові ідеї (в межах похибки):")
            for o, i, w, d in near:
                print(f"  -{o} +{i} ({d:+.1%})")


if __name__ == "__main__":
    main()
