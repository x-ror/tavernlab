"""A tier list the simulator can actually justify.

This is **not** the ladder. TavernLab does not scrape HSReplay or
Untapped for winrates (design U24: ToS + $0 budget), so it cannot tell
you what is strong in ranked play. What it can do is play the gauntlet
against itself and report the result, which is a different claim and has
to be labelled as one.

Every deck plays every other deck (mirrors excluded — a deck's standing
against the field should not include itself), and the tier is a band of
its average winrate across that field.

The caveats are not decoration. The AI is scripted, so combo and
value-engine decks come out **below** their real strength, and the field
is twelve decks rather than a ladder's long tail. `tier_caveats()` is
the list the UI must print next to the table.
"""
import math

from hs2 import formats
from hs2.sim import run_matrix, winrate

# Bands on "win rate against this field". Stated here rather than buried
# in the UI so the number and its label can never disagree.
TIERS = (
    ("S", 0.56),
    ("A", 0.52),
    ("B", 0.48),
    ("C", 0.44),
    ("D", 0.0),
)


def margin(n_games):
    """95% half-width on one matchup, in win-rate points.

    At 20 games a pair this is ±22 points: the 100% and 20% matchups
    such a run produces are noise wearing a number's clothes. Reporting
    it is the difference between a tier list and a horoscope.
    """
    if n_games <= 0:
        return 1.0
    return round(1.96 * math.sqrt(0.25 / n_games), 4)


def tier_for(rate):
    for name, floor in TIERS:
        if rate >= floor:
            return name
    return TIERS[-1][0]


def build(gauntlet, n_games=200, processes=14, fmt=formats.STANDARD,
          log=None):
    """Play the field against itself; return decks ranked by winrate.

    `n_games` is per ordered pair, so the cost is
    `len(gauntlet)^2 * n_games` games — quadratic, and the reason this
    is a job the user starts rather than something that runs on load.
    """
    names = [d.name for d in gauntlet]
    if log:
        pairs = len(names) * (len(names) - 1)
        log(f"Матриця {len(names)}×{len(names)}: "
            f"{pairs * n_games} боїв…")
    res = run_matrix(gauntlet, gauntlet, n_games, processes=processes,
                     fmt=fmt)

    rows = []
    for deck in gauntlet:
        against = {}
        for other in gauntlet:
            if other.name == deck.name:
                continue          # mirrors say nothing about standing
            against[other.name] = round(
                winrate(res, (deck.name, other.name)), 4)
        avg = sum(against.values()) / len(against) if against else 0.0
        rows.append({"name": deck.name, "cls": deck.cls,
                     "archetype": deck.archetype,
                     "winrate": round(avg, 4),
                     "tier": tier_for(avg),
                     "vs": against})
    rows.sort(key=lambda r: -r["winrate"])
    if log:
        log("Готово.")
    return {"format": fmt, "games_per_pair": n_games,
            "margin": margin(n_games),
            "tiers": [{"tier": t, "floor": f} for t, f in TIERS],
            "decks": rows}


def tier_caveats():
    """What this table is not. The UI prints these verbatim."""
    return ["meta.caveat_not_ladder", "meta.caveat_scripted_ai",
            "meta.caveat_small_field"]
