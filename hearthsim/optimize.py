"""Deck optimizer: hill-climbing card swaps evaluated vs the meta gauntlet.

For each candidate deck we measure the baseline winrate vs all meta decks,
then repeatedly propose swaps (cut one card, add one from the class pool)
and keep changes that measurably improve the gauntlet winrate."""
import random

from .decks import Deck, META, POOLS, DB
from .sim import gauntlet_winrate


LEGENDARY = {"Dr. Boom", "Sylvanas Windrunner", "Cairne Bloodhoof",
             "Ragnaros the Firelord", "Alexstrasza", "Tirion Fordring",
             "Grommash Hellscream", "Archmage Antonidas", "Edwin VanCleef",
             "Leeroy Jenkins", "Al'Akir the Windlord", "Bloodmage Thalnos"}


def propose_swap(deck, rng, seen):
    """One (out_name, in_name) proposal legal for the CURRENT deck."""
    counts = dict(deck.cardlist)
    pool = POOLS[deck.cls]
    outs = list(counts)
    ins = [c for c in pool if counts.get(c, 0) < (1 if c in LEGENDARY else 2)
           and not DB[c].token]
    for _ in range(200):
        o = rng.choice(outs)
        i = rng.choice(ins)
        if o != i and (o, i) not in seen:
            seen.add((o, i))
            return o, i
    return None


def optimize_deck(deck, n_eval=250, rounds=2, proposals=10, processes=14,
                  seed=17, log=print):
    rng = random.Random(seed)
    base, base_detail = gauntlet_winrate(deck, META, n_eval,
                                         processes=processes)
    log(f"[{deck.name}] baseline {base:.3f}")
    current = deck
    history = []
    for r in range(rounds):
        improved = False
        seen = set()
        for _ in range(proposals):
            prop = propose_swap(current, rng, seen)
            if prop is None:
                break
            o, i = prop
            try:
                cand = current.copy_with_swap(o, i)
            except AssertionError:
                continue
            wr, _ = gauntlet_winrate(cand, META, n_eval,
                                     processes=processes)
            delta = wr - base
            history.append((o, i, wr, delta))
            if delta > 0.015:  # require a clear improvement
                log(f"  round {r}: -{o} +{i}: {base:.3f} -> {wr:.3f} KEEP")
                current, base = cand, wr
                improved = True
            else:
                log(f"  round {r}: -{o} +{i}: {wr:.3f} (d={delta:+.3f})")
        if not improved:
            break
    return current, base, history
