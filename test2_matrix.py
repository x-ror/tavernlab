import sys
import time
sys.path.insert(0, ".")
from hs2 import carddata, decks
carddata.build_defs()
from hs2.sim import run_matrix, winrate
from hs2.engine import Game
from hs2.ai import Agent

if __name__ == "__main__":
    metas = decks.load_meta()
    # mechanics sanity
    turns, quests_done, heralds, corpses = [], 0, 0, 0
    for i in range(120):
        a, b = metas[i % 11], metas[(i * 3 + 1) % 11]
        g = Game(a, b, seed=i, agents=[Agent(a.archetype),
                                       Agent(b.archetype)])
        g.run()
        turns.append(g.turn)
        for p in g.players:
            if p.quest and p.quest.get("done"):
                quests_done += 1
            heralds += p.herald
            corpses += p.corpses
    print(f"avg len {sum(turns)/len(turns)/2:.1f} rounds, max "
          f"{max(turns)//2}, quests done {quests_done}, heralds {heralds}")

    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    t0 = time.time()
    res = run_matrix(metas, metas, n, processes=14)
    print(f"11x11 x{n}: {time.time()-t0:.0f}s")
    names = [d.name for d in metas]
    print(f"{'':22}" + "".join(f"{x.split()[0][:8]:>9}" for x in names))
    rows = []
    for a in names:
        rs = [winrate(res, (a, b)) for b in names]
        rows.append((sum(rs) / len(rs), a, rs))
    for avg, a, rs in sorted(rows, reverse=True):
        print(f"{a[:22]:22}" + "".join(f"{r:9.2f}" for r in rs) +
              f"  avg={avg:.3f}")
