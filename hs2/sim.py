"""Match simulation for engine v2."""
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


def play_match(deck_a, deck_b, n_games, base_seed=0):
    wa = wb = dr = 0
    agents = [Agent(deck_a.archetype), Agent(deck_b.archetype)]
    for i in range(n_games):
        g = Game(deck_a, deck_b, seed=base_seed * 1_000_003 + i,
                 agents=agents)
        w = g.run()
        if w == 0:
            wa += 1
        elif w == 1:
            wb += 1
        else:
            dr += 1
    return wa, wb, dr


def _worker(args):
    da, db, n, seed = args
    wa, wb, dr = play_match(da, db, n, base_seed=seed)
    return (da.name, db.name, wa, wb, dr)


def run_matrix(decks_a, decks_b, n_games, processes=14, chunk=500,
               fmt=formats.STANDARD):
    jobs = []
    seed = 1
    for da in decks_a:
        for db in decks_b:
            done = 0
            while done < n_games:
                k = min(chunk, n_games - done)
                jobs.append((da, db, k, seed))
                seed += 1
                done += k
    results = {}
    with Pool(processes, initializer=_init_worker,
              initargs=(fmt,)) as pool:
        for ai, bi, wa, wb, dr in pool.imap_unordered(_worker, jobs,
                                                      chunksize=1):
            key = (ai, bi)
            w0, w1, d0 = results.get(key, (0, 0, 0))
            results[key] = (w0 + wa, w1 + wb, d0 + dr)
    return results


def gauntlet_winrate(deck, gauntlet, n_per_opp, processes=14,
                     fmt=formats.STANDARD):
    res = run_matrix([deck], gauntlet, n_per_opp, processes=processes,
                     chunk=max(50, n_per_opp // processes), fmt=fmt)
    rates = {}
    for gd in gauntlet:
        wa, wb, dr = res[(deck.name, gd.name)]
        rates[gd.name] = (wa + 0.5 * dr) / (wa + wb + dr)
    avg = sum(rates.values()) / len(rates)
    return avg, rates


def winrate(res, key):
    wa, wb, dr = res[key]
    tot = wa + wb + dr
    return (wa + dr * 0.5) / tot if tot else 0.0
