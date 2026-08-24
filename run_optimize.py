"""Stage 1: optimize all candidate decks vs the meta gauntlet."""
import json
import sys
import time

from hearthsim import decks as D
from hearthsim.optimize import optimize_deck

if __name__ == "__main__":
    n_eval = int(sys.argv[1]) if len(sys.argv) > 1 else 250
    out = {}
    t0 = time.time()
    for deck in D.CANDIDATES:
        t1 = time.time()
        best, wr, hist = optimize_deck(deck, n_eval=n_eval, rounds=2,
                                       proposals=10, processes=14)
        out[deck.name] = {
            "cls": deck.cls,
            "archetype": deck.archetype,
            "original": deck.cardlist,
            "final": best.cardlist,
            "gauntlet_winrate": wr,
            "history": [(o, i, w, d) for o, i, w, d in hist],
        }
        print(f"== {deck.name}: {wr:.3f} ({time.time()-t1:.0f}s)",
              flush=True)
    with open("optimized.json", "w") as f:
        json.dump(out, f, indent=1)
    print(f"total {time.time()-t0:.0f}s")
