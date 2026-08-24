"""Print a readable summary of results2.json + optimized2.json."""
import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def wr(v):
    wa, wb, dr = v
    t = wa + wb + dr
    return (wa + 0.5 * dr) / t


if __name__ == "__main__":
    res = json.load(open(os.path.join(ROOT, "results2.json")))
    opt = json.load(open(os.path.join(ROOT, "optimized2.json")))
    meta_names = sorted({k.split("||")[0]
                         for k in res["meta_vs_meta"]})
    mine_names = sorted({k.split("||")[0]
                         for k in res["mine_vs_meta"]})

    print("=== META BASELINE (avg winrate in meta 10x10, 10k games) ===")
    rows = []
    for a in meta_names:
        rs = [wr(res["meta_vs_meta"][f"{a}||{b}"]) for b in meta_names]
        rows.append((sum(rs) / len(rs), a))
    for r, a in sorted(rows, reverse=True):
        print(f"  {r:.3f}  {a}")

    print("\n=== MY DECKS vs META GAUNTLET (10k games per pairing) ===")
    rows = []
    for a in mine_names:
        rs = {b: wr(res["mine_vs_meta"][f"{a}||{b}"]) for b in meta_names}
        avg = sum(rs.values()) / len(rs)
        rows.append((avg, a, rs))
    for avg, a, rs in sorted(rows, reverse=True):
        worst = min(rs, key=rs.get)
        best = max(rs, key=rs.get)
        print(f"  {avg:.3f}  {a}")
        print(f"          best: {rs[best]:.2f} vs {best.split(' [')[0]}, "
              f"worst: {rs[worst]:.2f} vs {worst.split(' [')[0]}")

    print("\n=== OPTIMIZER: accepted swaps ===")
    for name, info in opt.items():
        kept = [(o, i, w, d) for o, i, w, d in info["history"] if d > 0.015]
        if kept:
            print(f"  {name}:")
            for o, i, w, d in kept:
                print(f"    -{o} +{i} ({d:+.3f} -> {w:.3f})")

    print("\n=== OPTIMIZER: best rejected ideas (hints to improve) ===")
    for name, info in opt.items():
        rej = sorted([h for h in info["history"] if h[3] <= 0.015],
                     key=lambda h: -h[3])[:2]
        if rej:
            hints = ", ".join(f"-{o}+{i} ({d:+.3f})" for o, i, w, d in rej)
            print(f"  {name}: {hints}")

    print("\n=== FULL MATRIX my x meta ===")
    short = [b.split(" [")[0][:9] for b in meta_names]
    print(f"{'':30}" + "".join(f"{s:>10}" for s in short))
    for avg, a, rs in sorted(rows, reverse=True):
        print(f"{a[:30]:30}" +
              "".join(f"{rs[b]:10.3f}" for b in meta_names))
