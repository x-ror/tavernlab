#!/usr/bin/env python3
"""Ігровий помічник: муліган, читання колоди суперника, журнал покращень.

Команди:
  python3 advisor.py stats   --deck "<код>" [--games 1500]
      Один раз прогнати інструментовані симуляції для ВАШОЇ колоди проти
      всіх мета-колод; результат кешується в advisor_cache/.

  python3 advisor.py mull    --deck "<код>" --opp priest \\
                             --hand "Frostbolt;Fireball;Vulcanos"
      Що лишити / що скинути в мулігані проти цього класу, з виміряним
      впливом на перемогу і поясненням.

  python3 advisor.py predict --opp rogue --seen "Preparation;Deja Vu"
      Наскільки зіграні суперником карти збігаються з відомою топ-колодою
      його класу + які загрози ще очікувати.

  python3 advisor.py coach   --deck "<код>"
      Записує у coach_log.md висновки: слабкі матчапи, карти-кандидати
      на заміну, найкращі кіпи.
"""
import argparse
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
if not getattr(sys, "frozen", False):
    os.chdir(os.path.dirname(os.path.abspath(__file__)))

import console
import paths
from hs2 import carddata, decks

# The same directory `app.py` uses, so the interface and the CLI
# share one cache instead of each building its own.
CACHE_DIR = paths.in_home("advisor_cache")
COACH_LOG = paths.in_home("coach_log.md")

CLS_ALIASES = {
    "mage": "MAGE", "маг": "MAGE", "hunter": "HUNTER", "мисливець": "HUNTER",
    "warrior": "WARRIOR", "воїн": "WARRIOR", "paladin": "PALADIN",
    "паладин": "PALADIN", "priest": "PRIEST", "жрець": "PRIEST",
    "прист": "PRIEST", "rogue": "ROGUE", "рога": "ROGUE",
    "shaman": "SHAMAN", "шаман": "SHAMAN", "warlock": "WARLOCK",
    "лок": "WARLOCK", "druid": "DRUID", "друїд": "DRUID",
    "dh": "DEMONHUNTER", "demonhunter": "DEMONHUNTER",
    "dk": "DEATHKNIGHT", "deathknight": "DEATHKNIGHT",
}


def deck_key(code):
    """Must agree with `app.deck_key`: both address one cache file."""
    from hs2 import deckstring
    try:
        code = deckstring.extract(code)
    except ValueError:
        code = code.strip()
    return hashlib.sha1(code.encode()).hexdigest()[:12]


def load_deck(code):
    from evaluate import resolve_deck
    return resolve_deck(code)


def meta_for_class(cls):
    return [d for d in decks.load_meta() if d.cls == cls]


def find_card(name):
    try:
        return carddata.get_def(name)
    except KeyError:
        # fuzzy: case-insensitive prefix
        low = name.lower()
        for d in carddata.DEFS.values():
            if d.name.lower() == low:
                return d
        cands = [d for d in carddata.DEFS.values()
                 if d.coll and d.name.lower().startswith(low)]
        if len(cands) == 1:
            return cands[0]
        raise SystemExit(f"Не знаю карту: {name!r}"
                         + (f" (варіанти: {[c.name for c in cands[:5]]})"
                            if cands else ""))


# ------------------------------------------------------------------ stats
def cmd_stats(args):
    from hs2.telemetry import build_stats
    deck = load_deck(args.deck)
    metas = decks.load_meta()
    os.makedirs(CACHE_DIR, exist_ok=True)
    t0 = time.time()
    stats = build_stats(deck, metas, n_per_opp=args.games,
                        processes=args.procs)
    path = os.path.join(CACHE_DIR, deck_key(args.deck) + ".json")
    from hs2.optimize import deck_counts
    json.dump({"code": args.deck.strip(), "cls": deck.cls,
               "deck_cards": sorted(deck_counts(deck)),
               "games_per_opp": args.games, "stats": stats},
              open(path, "w", encoding="utf-8"), ensure_ascii=False)
    total = sum(s["games"] for s in stats.values())
    print(f"{total} інструментованих боїв за {time.time()-t0:.0f}с "
          f"-> {path}")
    for name, s in sorted(stats.items(),
                          key=lambda kv: kv[1]["wins"] / kv[1]["games"]):
        print(f"  {s['wins']/s['games']:6.1%}  {name}")


def load_stats(code):
    path = os.path.join(CACHE_DIR, deck_key(code) + ".json")
    if not os.path.exists(path):
        raise SystemExit("Спершу побудуйте статистику: "
                         f"python3 advisor.py stats --deck \"{code[:20]}…\"")
    return json.load(open(path, encoding="utf-8"))


# ---------------------------------------------------------------- mulligan
def _reason(card, opp_deck, delta):
    r = []
    a = opp_deck.archetype
    if card.cost >= 6:
        r.append(f"дорога ({card.cost} мани)")
        if a == "aggro":
            r.append("проти агресії рука мусить бути дешевою")
    elif card.cost <= 2:
        r.append("дешева, тримає темп перших ходів")
    if card.ai_hint and card.ai_hint[0] == "dmg":
        if a == "aggro":
            r.append("ремувал цінний проти їхніх ранніх міньйонів")
        elif a == "control":
            r.append("проти контролю пряма шкода краще йде в лице пізніше")
    if card.type == "MINION" and 2 <= card.cost <= 4 and a != "aggro":
        r.append("тіло на криву — база темпу")
    if delta is not None:
        r.append(f"виміряно: у стартовій руці змінює шанс на {delta:+.1%}")
    return "; ".join(r) if r else "нейтральна за даними симуляцій"


def cmd_mull(args):
    carddata.build_defs()
    data = load_stats(args.deck)
    cls = CLS_ALIASES.get(args.opp.lower(), args.opp.upper())
    opps = meta_for_class(cls)
    if not opps:
        raise SystemExit(f"Немає мета-колоди класу {cls}")
    hand = [h.strip() for h in args.hand.split(";") if h.strip()]
    print(f"Муліган проти {opps[0].name} ({cls.title()}):\n")
    for hname in hand:
        card = find_card(hname)
        deltas, ns = [], 0
        for opp in opps:
            s = data["stats"].get(opp.name)
            if not s:
                continue
            base = s["wins"] / s["games"]
            rec = s["cards"].get(card.name)
            if rec and rec[0] >= 30:
                deltas.append(rec[1] / rec[0] - base)
                ns += rec[0]
        delta = sum(deltas) / len(deltas) if deltas else None
        keep = (delta is not None and delta > -0.01) if delta is not None \
            else card.cost <= 3
        verdict = "ЛИШИТИ " if keep else "СКИНУТИ"
        dtxt = f"{delta:+.1%} ({ns} ігор)" if delta is not None \
            else "мало даних"
        print(f"  {verdict}  {card.name:<28} {dtxt}")
        print(f"           └ {_reason(card, opps[0], delta)}")
    print("\nБазовий вінрейт матчапу: " + ", ".join(
        f"{data['stats'][o.name]['wins']/data['stats'][o.name]['games']:.0%}"
        f" vs {o.name}" for o in opps if o.name in data["stats"]))


# ----------------------------------------------------------------- predict
def cmd_predict(args):
    carddata.build_defs()
    cls = CLS_ALIASES.get(args.opp.lower(), args.opp.upper())
    opps = meta_for_class(cls)
    seen = [find_card(s.strip()).name
            for s in args.seen.split(";") if s.strip()]
    print(f"Суперник: {cls.title()}. Зіграв: {', '.join(seen) or '—'}\n")
    for opp in opps:
        from hs2.optimize import deck_counts
        counts = deck_counts(opp)
        hit = sum(1 for s in seen if s in counts)
        frac = hit / len(seen) if seen else 1.0
        print(f"Схожість з «{opp.name}»: {hit}/{len(seen)} карт "
              f"({frac:.0%})")
        if frac >= 0.5:
            unseen = [(cn, carddata.get_def(cn).cost) for cn in counts
                      if cn not in seen]
            unseen.sort(key=lambda x: -x[1])
            print("  Очікуй загрози (ще не бачені, найдорожчі):")
            for cn, cost in unseen[:8]:
                d = carddata.get_def(cn)
                extra = ""
                if d.text:
                    extra = " — " + d.text[:70]
                print(f"    ({cost}) {cn}{extra}")
        else:
            print("  Мало збігів — можливо, нестандартний лист.")


# ------------------------------------------------------------------- coach
def cmd_coach(args):
    carddata.build_defs()
    data = load_stats(args.deck)
    lines = [f"# Coach log — {time.strftime('%Y-%m-%d %H:%M')}",
             f"Колода: {data['cls'].title()} "
             f"({data['games_per_opp']} ігор на матчап)", ""]
    stats = data["stats"]
    ranked = sorted(stats.items(),
                    key=lambda kv: kv[1]["wins"] / kv[1]["games"])
    lines.append("## Слабкі матчапи (тренуй або техни проти них)")
    for name, s in ranked[:3]:
        lines.append(f"- {s['wins']/s['games']:.0%} проти {name}")
    # per-card drawn deltas averaged over matchups
    own = set(data.get("deck_cards", []))
    per_card = {}
    for name, s in stats.items():
        base = s["wins"] / s["games"]
        for cn, (on, ow, dn, dw) in s["cards"].items():
            if dn >= 100 and (not own or cn in own):
                per_card.setdefault(cn, []).append(dw / dn - base)
    avg = {cn: sum(v) / len(v) for cn, v in per_card.items() if v}
    worst = sorted(avg.items(), key=lambda kv: kv[1])[:5]
    best = sorted(avg.items(), key=lambda kv: -kv[1])[:5]
    lines.append("\n## Карти, що тягнуть униз (кандидати на заміну)")
    for cn, d in worst:
        lines.append(f"- {cn}: {d:+.1%} коли дотягнута")
    lines.append("\n## Найцінніші карти (муліганьте на них)")
    for cn, d in best:
        lines.append(f"- {cn}: {d:+.1%} коли дотягнута")
    lines.append("")
    with open(COACH_LOG, "a", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"-> дописано в {COACH_LOG}")


if __name__ == "__main__":
    console.init()
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("stats")
    s.add_argument("--deck", required=True)
    s.add_argument("--games", type=int, default=1500)
    s.add_argument("--procs", type=int, default=14)
    m = sub.add_parser("mull")
    m.add_argument("--deck", required=True)
    m.add_argument("--opp", required=True)
    m.add_argument("--hand", required=True)
    p = sub.add_parser("predict")
    p.add_argument("--opp", required=True)
    p.add_argument("--seen", default="")
    c = sub.add_parser("coach")
    c.add_argument("--deck", required=True)
    args = ap.parse_args()
    {"stats": cmd_stats, "mull": cmd_mull, "predict": cmd_predict,
     "coach": cmd_coach}[args.cmd](args)
