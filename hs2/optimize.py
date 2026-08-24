"""Candidate deck construction + hill-climbing optimizer.

The pool follows whichever format `carddata` currently holds, so the same
optimizer serves Standard and Wild without knowing which it is in.
"""
import random

from . import carddata, formats
from .decks import Deck
from .sim import gauntlet_winrate

CLASS_OF = {
    "DEATHKNIGHT": "UUB Egg Death Knight",
}


def class_pool(cls):
    """Buildable cards for a class, one entry per card name.

    Deduplication matters once Wild is loaded: ~1000 names are printed in
    more than one set, and `build_seed` keys its counts by name while
    counting every def it walks. Two printings of the same card then
    overwrite one entry but advance the total twice, and the seed comes
    out of the loop three cards short of a legal deck.
    """
    pool = carddata.standard_pool(
        lambda d: d.cls in (cls, "NEUTRAL") and
        d.type in ("MINION", "SPELL", "WEAPON", "LOCATION"))
    seen, out = set(), []
    for d in pool:
        if d.name in seen:
            continue
        seen.add(d.name)
        out.append(d)
    return out


def card_quality(d):
    """Crude standalone-quality heuristic for seed construction."""
    q = 0.0
    if d.type == "MINION":
        q = (d.atk + d.hp) - 2 * d.cost + 1
        for kw in ("taunt", "rush", "charge", "divine_shield", "lifesteal",
                   "windfury"):
            if getattr(d, kw):
                q += 0.7
        if d.battlecry or d.deathrattle or d.triggers or d.aura:
            q += 1.2
    elif d.type == "SPELL":
        q = 0.5 + (1.5 if d.ai_hint else 0) + (1.0 if d.spell else 0)
    elif d.type == "WEAPON":
        q = d.atk * d.dur - d.cost
    elif d.type == "LOCATION":
        q = 1.5
    return q


CURVE = {0: 1, 1: 4, 2: 8, 3: 6, 4: 4, 5: 3, 6: 2, 7: 1, 8: 1}


def build_seed(cls, name, rng, avoid=()):
    pool = [d for d in class_pool(cls) if d.name not in avoid]
    by_cost = {}
    for d in pool:
        by_cost.setdefault(min(d.cost, 8), []).append(d)
    cards = {}
    total = 0
    for cost, want in CURVE.items():
        cands = sorted(by_cost.get(cost, []),
                       key=lambda d: -card_quality(d))
        picked = 0
        for d in cands:
            if picked >= want or total >= 30:
                break
            n = 1 if d.rarity == "LEGENDARY" else 2
            n = min(n, 30 - total)
            if n <= 0:
                break
            cards[d.name] = n
            total += n
            picked += n
    # fill remaining with best leftovers
    leftovers = sorted(pool, key=lambda d: -card_quality(d))
    for d in leftovers:
        if total >= 30:
            break
        cur = cards.get(d.name, 0)
        cap = 1 if d.rarity == "LEGENDARY" else 2
        if cur < cap:
            cards[d.name] = cur + 1
            total += 1
    return Deck.from_names(name, cls, "midrange", sorted(cards.items()))


def deck_counts(deck):
    counts = {}
    for cid in deck.card_ids:
        n = carddata.DEFS[cid].name
        counts[n] = counts.get(n, 0) + 1
    return counts


def swap(deck, out_name, in_name):
    counts = deck_counts(deck)
    counts[out_name] -= 1
    if counts[out_name] == 0:
        del counts[out_name]
    counts[in_name] = counts.get(in_name, 0) + 1
    return Deck.from_names(deck.name, deck.cls, deck.archetype,
                           sorted(counts.items()))


def propose(deck, rng, seen):
    counts = deck_counts(deck)
    pool = class_pool(deck.cls)
    outs = list(counts)
    ins = [d.name for d in pool
           if counts.get(d.name, 0) <
           (1 if d.rarity == "LEGENDARY" else 2)]
    for _ in range(300):
        o = rng.choice(outs)
        i = rng.choice(ins)
        if o != i and (o, i) not in seen:
            seen.add((o, i))
            return o, i
    return None


def optimize(deck, gauntlet, n_eval=200, rounds=2, proposals=10,
             processes=14, seed=11, log=print):
    rng = random.Random(seed)
    fmt = carddata.LOADED_FORMAT or formats.STANDARD
    base, _ = gauntlet_winrate(deck, gauntlet, n_eval,
                               processes=processes, fmt=fmt)
    log(f"[{deck.name}] baseline {base:.3f}")
    current = deck
    history = []
    for r in range(rounds):
        improved = False
        seen = set()
        for _ in range(proposals):
            prop = propose(current, rng, seen)
            if prop is None:
                break
            o, i = prop
            try:
                cand = swap(current, o, i)
            except (ValueError, KeyError):
                continue
            wr, _ = gauntlet_winrate(cand, gauntlet, n_eval, fmt=fmt,
                                     processes=processes)
            delta = wr - base
            history.append((o, i, wr, delta))
            if delta > 0.015:
                log(f"  -{o} +{i}: {base:.3f}->{wr:.3f} KEEP")
                current, base = cand, wr
                improved = True
            else:
                log(f"  -{o} +{i}: {wr:.3f} ({delta:+.3f})")
        if not improved:
            break
    return current, base, history
