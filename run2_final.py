"""Stage 2 (Standard): final 10,000-game matrices."""
import json
import sys
import time
sys.path.insert(0, ".")

from hs2 import carddata, decks
carddata.build_defs()
from hs2.decks import Deck
from hs2.sim import run_matrix

N = 10_000
PROCS = 15

if __name__ == "__main__":
    metas = decks.load_meta()
    opt = json.load(open("optimized2.json"))
    mine = []
    for name, info in opt.items():
        mine.append(Deck.from_names(name, info["cls"], info["archetype"],
                                    [tuple(x) for x in info["cards"]]))
    t0 = time.time()
    res_mine = run_matrix(mine, metas, N, processes=PROCS, chunk=1000)
    print(f"mine x meta ({len(mine)}x{len(metas)}x{N}) "
          f"in {time.time()-t0:.0f}s", flush=True)
    t1 = time.time()
    res_meta = run_matrix(metas, metas, N, processes=PROCS, chunk=1000)
    print(f"meta x meta in {time.time()-t1:.0f}s", flush=True)
    out = {
        "n_games": N,
        "mine_vs_meta": {f"{a}||{b}": v for (a, b), v in res_mine.items()},
        "meta_vs_meta": {f"{a}||{b}": v for (a, b), v in res_meta.items()},
        "decks": {},
    }
    for d in mine:
        from hs2.optimize import deck_counts
        out["decks"][d.name] = {"cls": d.cls, "archetype": d.archetype,
                                "cards": sorted(deck_counts(d).items())}
    for d in metas:
        from hs2.optimize import deck_counts
        out["decks"][d.name] = {"cls": d.cls, "archetype": d.archetype,
                                "cards": sorted(deck_counts(d).items())}
    with open("results2.json", "w") as f:
        json.dump(out, f, indent=1)
    print(f"total {time.time()-t0:.0f}s -> results2.json")
