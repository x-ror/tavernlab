"""Stage 2: final 10,000-game matrices.

- optimized candidate decks (20) x meta gauntlet (10), 10k games each pair
- meta x meta baseline, 10k games each pair
"""
import json
import time

from hearthsim import decks as D
from hearthsim.decks import Deck
from hearthsim.sim import run_matrix

N = 10_000
PROCS = 15

if __name__ == "__main__":
    with open("optimized.json") as f:
        opt = json.load(f)
    mine = []
    for name, info in opt.items():
        mine.append(Deck(name, info["cls"], info["archetype"],
                         [tuple(x) for x in info["final"]]))

    t0 = time.time()
    res_mine = run_matrix(mine, D.META, N, processes=PROCS, chunk=1000)
    print(f"mine x meta ({len(mine)}x{len(D.META)}x{N}) "
          f"in {time.time()-t0:.0f}s", flush=True)

    t1 = time.time()
    res_meta = run_matrix(D.META, D.META, N, processes=PROCS, chunk=1000)
    print(f"meta x meta in {time.time()-t1:.0f}s", flush=True)

    out = {
        "n_games": N,
        "mine_vs_meta": {f"{a}||{b}": v for (a, b), v in res_mine.items()},
        "meta_vs_meta": {f"{a}||{b}": v for (a, b), v in res_meta.items()},
        "decks": {d.name: {"cls": d.cls, "archetype": d.archetype,
                           "cards": d.cardlist}
                  for d in mine + D.META},
    }
    with open("results.json", "w") as f:
        json.dump(out, f, indent=1)
    print(f"total {time.time()-t0:.0f}s -> results.json")
