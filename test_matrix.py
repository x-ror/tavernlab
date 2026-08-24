import sys
import time
from hearthsim import decks as D
from hearthsim.sim import run_matrix, winrate

if __name__ == "__main__":
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    t0 = time.time()
    res = run_matrix(D.META, D.META, n, processes=14)
    print(f"meta 10x10 x{n} in {time.time()-t0:.0f}s")
    names = [d.name for d in D.META]
    short = [x.split(" [")[0][:9] for x in names]
    print(f"{'':15}" + f"{'avg':>7}" + "".join(f"{s:>10}" for s in short))
    for a in names:
        rs = [winrate(res, (a, b)) for b in names]
        avg = sum(rs) / len(rs)
        print(f"{a.split(' [')[0]:15}" + f"{avg:7.3f}" +
              "".join(f"{r:10.2f}" for r in rs))
