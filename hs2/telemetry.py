"""Instrumented simulations: per-card win-rate impact per matchup.

For every game we record, for player 0: the post-mulligan opening hand,
every card drawn during the game, and the result. Aggregated per opponent
this yields HSReplay-style stats: "kept in opening hand" win rate and
"drawn during game" win rate vs the matchup baseline.
"""
import multiprocessing

try:
    _ctx = multiprocessing.get_context("fork")
except ValueError:          # Windows: no fork
    _ctx = multiprocessing.get_context("spawn")
Pool = _ctx.Pool

from . import formats
from .engine import Game
from .ai import Agent


def _init_worker(fmt):
    """Build the card defs in a spawned worker (Windows has no fork,
    so nothing the parent built is inherited).

    The format is an argument because the worker cannot see the
    decks: a Wild matrix needs the Wild corpus or every card id in
    it is a KeyError.
    """
    from . import carddata
    carddata.ensure_defs(fmt)


class TelemetryGame(Game):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.drawn0 = set()

    def draw(self, p, n=1, filt=None):
        inst = super().draw(p, n, filt)
        if p.idx == 0:
            for i in p.hand:
                self.drawn0.add(i.card.name)
        return inst


def play_instrumented(deck, opp, n_games, base_seed=0):
    agents = [Agent(deck.archetype), Agent(opp.archetype)]
    agg = {}   # card -> [open_n, open_w, drawn_n, drawn_w]
    wins = games = 0
    for i in range(n_games):
        g = TelemetryGame(deck, opp, seed=base_seed * 999_983 + i,
                          agents=agents)
        w = g.run()
        if w is None:
            continue
        games += 1
        won = 1 if w == 0 else 0
        wins += won
        p0 = g.players[0]
        opening = {c.name for c in p0.marks.get("start_hand", [])}
        drawn = g.drawn0 | opening
        for cn in drawn:
            a = agg.setdefault(cn, [0, 0, 0, 0])
            a[2] += 1
            a[3] += won
            if cn in opening:
                a[0] += 1
                a[1] += won
    return {"games": games, "wins": wins, "cards": agg}


def _worker(args):
    deck, opp, n, seed = args
    return (opp.name, play_instrumented(deck, opp, n, base_seed=seed))


def build_stats(deck, gauntlet, n_per_opp=1500, processes=14,
                fmt=formats.STANDARD):
    jobs = [(deck, opp, n_per_opp, i + 1)
            for i, opp in enumerate(gauntlet)]
    out = {}
    with Pool(processes, initializer=_init_worker,
              initargs=(fmt,)) as pool:
        for name, res in pool.imap_unordered(_worker, jobs):
            out[name] = res
    return out
