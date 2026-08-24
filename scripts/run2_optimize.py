"""Stage 1 (Standard): build + optimize 2 candidate decks per class."""
import json
import os
import random
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from hs2 import carddata, decks
carddata.build_defs()
from hs2.optimize import build_seed, optimize, deck_counts
from hs2.decks import Deck

if __name__ == "__main__":
    n_eval = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    metas = decks.load_meta()
    rng = random.Random(5)
    out = {}
    t0 = time.time()
    for meta in metas:
        cls = meta.cls
        # candidate 1: tuned version of the tracker deck
        tuned = Deck(f"{meta.name.replace(' [meta]','')} tuned [mine]",
                     cls, meta.archetype, list(meta.card_ids))
        tuned.name = f"{meta.name} tuned [mine]"
        # candidate 2: fresh build from the implemented Standard pool
        fresh = build_seed(cls, f"{cls.title()} Custom [mine]", rng)
        for deck in (tuned, fresh):
            t1 = time.time()
            best, wr, hist = optimize(deck, metas, n_eval=n_eval,
                                      rounds=2, proposals=10,
                                      processes=14)
            out[deck.name] = {
                "cls": cls,
                "archetype": deck.archetype,
                "cards": sorted(deck_counts(best).items()),
                "gauntlet_winrate": wr,
                "history": hist,
            }
            print(f"== {deck.name}: {wr:.3f} ({time.time()-t1:.0f}s)",
                  flush=True)
    with open(os.path.join(ROOT, "optimized2.json"), "w") as f:
        json.dump(out, f, indent=1)
    print(f"total {time.time()-t0:.0f}s")
