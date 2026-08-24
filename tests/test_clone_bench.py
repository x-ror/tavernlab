"""Clone speed. **Should**, not a merge gate (design §4.5).

The gate for PR 2 is `tests/test_clone.py`'s identity tests; a fast clone
that shares Irida's void with the original is worse than a slow correct
one. This prints the number and only fails if clone has become so slow it
would break the <200 ms 1-ply budget outright.
"""
import time

import pytest

from conftest import new_game

TARGET_US = 100.0      # design target
CEILING_US = 2000.0    # 40 actions * this must still fit 1-ply's 200 ms


@pytest.fixture(scope="module")
def mid_game(metas):
    g = new_game(metas[0], metas[4], seed=17)
    g.start(first=0)
    for t in range(1, 9):
        g.turn = t
        p = g.players[g.current]
        g.begin_turn(p)
        if g.over:
            break
        g.agents[p.idx].take_turn(g, p)
        if g.over:
            break
        g.end_turn(p)
        g.current = 1 - g.current
    g._ensure_eids()
    return g


def test_clone_is_fast_enough_for_one_ply(mid_game, capsys):
    n = 300
    mid_game.clone()                      # warm the eid sweep
    t0 = time.perf_counter()
    for _ in range(n):
        mid_game.clone()
    per_us = (time.perf_counter() - t0) / n * 1e6
    with capsys.disabled():
        print(f"\n  clone: {per_us:.0f} µs "
              f"({len(mid_game._by_eid)} entities), "
              f"target {TARGET_US:.0f} µs")
    assert per_us < CEILING_US, (
        f"{per_us:.0f} µs per clone breaks the 1-ply budget; profile "
        f"before reaching for Rust (design §4.5)")


def test_legal_actions_is_under_half_a_millisecond(mid_game, capsys):
    from hs2.actions import legal_actions
    p = mid_game.players[0]
    legal_actions(mid_game, p)
    n = 200
    t0 = time.perf_counter()
    for _ in range(n):
        acts, _complete = legal_actions(mid_game, p)
    per_us = (time.perf_counter() - t0) / n * 1e6
    with capsys.disabled():
        print(f"  legal_actions: {per_us:.0f} µs ({len(acts)} actions)")
    assert per_us < 5000.0
