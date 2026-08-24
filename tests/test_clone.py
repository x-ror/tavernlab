"""PR 2 merge gate: `Game.clone()` identity.

These are the tests the design names as blocking; the <100 µs clone budget
is a Should and lives in `test_clone_bench.py`.
"""
import pytest

from conftest import build_deck, new_game

from hs2 import carddata
from hs2.actions import Action, apply, legal_actions
from hs2.ai import Agent
from hs2.engine import CardInst, Location, Minion, Player, Weapon, Game


# --------------------------------------------------------------- universe
def test_every_entity_is_copied_with_same_eid_and_new_identity(metas):
    g = new_game(metas[0], metas[1], seed=11)
    g.run()
    g2 = new_game(metas[0], metas[1], seed=11)
    g2.start(first=0)
    for t in range(1, 9):
        g2.turn = t
        p = g2.players[g2.current]
        g2.begin_turn(p)
        if g2.over:
            break
        g2.agents[p.idx].take_turn(g2, p)
        g2.end_turn(p)
        g2.current = 1 - g2.current
    g2._ensure_eids()
    c = g2.clone()
    for eid, obj in g2._by_eid.items():
        assert eid in c._by_eid, f"eid {eid} missing from clone"
        assert c._by_eid[eid] is not obj, f"eid {eid} shared with original"
        assert c._by_eid[eid].eid == eid
        assert type(c._by_eid[eid]) is type(obj)


def test_zone_lists_are_rebuilt_not_shared(metas):
    g = new_game(metas[2], metas[3], seed=5)
    g.start(first=0)
    g.turn = 1
    g.begin_turn(g.players[0])
    c = g.clone()
    for p, q in zip(g.players, c.players):
        for zone in ("hand", "deck", "board", "void"):
            a, b = getattr(p, zone), getattr(q, zone)
            assert a is not b
            assert [x.eid for x in a] == [x.eid for x in b]
            for x in b:
                assert x is c._by_eid[x.eid]
    c.players[0].hand.pop()
    assert len(c.players[0].hand) != len(g.players[0].hand)


# ------------------------------------------------------------------ Irida
def test_irida_tick_fires_on_clone_not_original(carddb):
    """Irida dumps the deck into `void`; the tick refills from it.

    Those CardInsts are in **no zone**, so a clone that copied zone lists
    only would leave the copy ticking the original's instances.
    """
    g = new_game(build_deck("irida", "DEMONHUNTER", []),
                 build_deck("opp", "MAGE", []), seed=3)
    g.start(first=0)
    g.turn = 1
    p = g.players[0]
    g.begin_turn(p)
    inst = g.reg(carddata.make_inst_by_name("Irida Sinseeker"))
    p.hand.append(inst)
    p.mana = 10
    g.play_card(p, inst)
    assert p.void, "Irida battlecry should fill void"
    assert p.turn_start_fx and p.turn_start_fx[0].handler_id == "irida_tick"

    void_before = list(p.void)
    n_before = len(p.void)
    c = g.clone()
    cp = c.players[0]

    assert cp.void is not p.void
    assert [i.eid for i in cp.void] == [i.eid for i in p.void]
    assert all(i.eid in c._by_eid for i in cp.void)
    assert all(c._by_eid[i.eid] is i for i in cp.void)

    c.turn = 2
    c.current = 0
    c.begin_turn(cp)

    assert len(cp.void) < n_before, "clone tick did not consume clone.void"
    assert p.void == void_before, "original.void was mutated by the clone"
    assert len(p.void) == n_before


# ---------------------------------------------------------------- Godfrey
def test_godfrey_overflow_insts_are_in_by_eid_after_clone(carddb):
    """Burned cards live only in `marks`; they must be eids, and the
    instances they name must survive the copy."""
    deck = build_deck("godfrey", "WARLOCK", ["Godfrey the Betrayer"])
    g = new_game(deck, build_deck("opp", "MAGE", []), seed=4)
    g.start(first=0)
    p = g.players[0]
    assert any(l.handler_id == "godfrey_overdraw" for l in p.listeners)

    while len(p.hand) < 10 and p.deck:
        p.hand.append(p.deck.pop())
    g.turn = 1
    burned_before = len(p.marks.get("overflow_eids", []))
    g.draw(p, 3)
    eids = p.marks.get("overflow_eids", [])
    assert len(eids) > burned_before, "overdraw did not bank an overflow"
    assert all(isinstance(e, int) for e in eids), \
        "overflow must be eids, not CardInst refs"

    c = g.clone()
    cp = c.players[0]
    assert cp.marks["overflow_eids"] == eids
    for e in eids:
        assert e in c._by_eid
        assert c._by_eid[e] is not g._by_eid[e]

    hand_before = list(p.hand)
    cp.hand = cp.hand[:2]
    c.turn = 2
    c.current = 0
    c.begin_turn(cp)
    assert p.hand == hand_before, "clone tick appended to original.hand"


# -------------------------------------------------------------- Warptooth
def test_warptooth_listen_does_not_mutate_original(carddb):
    deck = build_deck("warp", "WARRIOR", ["Warptooth"])
    g = new_game(deck, build_deck("opp", "MAGE", []), seed=6)
    g.start(first=0)
    p = g.players[0]
    ids = {l.handler_id for l in p.listeners}
    assert {"warptooth_hero_dmg", "warptooth_minion_dmg"} <= ids
    assert all(isinstance(l.handler_id, str) for l in p.listeners), \
        "listeners must be data, never closures"

    g.turn = 1
    g.current = 0
    c = g.clone()
    cp = c.players[0]
    assert cp.listeners is not p.listeners
    assert cp.listeners[0] is not p.listeners[0]

    for _ in range(4):
        c.deal_damage(None, cp, 1)
    assert cp.marks.get("warptooth_dmg", 0) == 4
    assert p.marks.get("warptooth_dmg", 0) == 0, \
        "clone listener fired against the original player"


def test_listener_source_resolves_through_the_clone(carddb):
    """`Shadow of Demise` keys off a specific CardInst by eid."""
    deck = build_deck("sod", "PRIEST", ["Shadow of Demise"])
    g = new_game(deck, build_deck("opp", "MAGE", []), seed=8)
    g.start(first=0)
    p = g.players[0]
    ls = [l for l in p.listeners if l.handler_id == "shadow_demise_watch"]
    assert ls, "start-of-game listener missing"
    src_eid = ls[0].source_eid
    assert g.by_eid(src_eid) is not None
    c = g.clone()
    cl = [l for l in c.players[0].listeners
          if l.handler_id == "shadow_demise_watch"][0]
    assert cl.source_eid == src_eid
    assert c.by_eid(src_eid) is not g.by_eid(src_eid)


# ----------------------------------------------------------------- action
def test_apply_action_uses_eid_not_id(metas):
    g = new_game(metas[4], metas[0], seed=9)
    g.start(first=0)
    g.turn = 1
    p = g.players[0]
    g.begin_turn(p)
    g2 = g.clone()
    acts, _ = legal_actions(g, p)
    plays = [a for a in acts if a.kind == "play"]
    if not plays:
        pytest.skip("no play available on turn 1 with this seed")
    a = plays[0]
    before = len(g2.players[0].hand)
    assert apply(g2, g2.players[0], a) is not False
    assert len(g2.players[0].hand) == before - 1
    assert len(g.players[0].hand) == before, "apply touched the original"


def test_action_is_json_round_trippable():
    a = Action("play", eid=17, target_eid=2, choice=1)
    assert Action.from_dict(a.to_dict()) == a
    assert "attacker_eid" not in a.to_dict()


# ------------------------------------------------------------- remainder
@pytest.mark.parametrize("seed", range(20))
def test_clone_run_matches_seed_path(metas, seed):
    """Clone at turn 3, finish both; the remainders must be identical."""
    a, b = metas[seed % len(metas)], metas[(seed + 5) % len(metas)]

    def fresh():
        g = new_game(a, b, seed=seed)
        g.start(first=seed % 2)
        return g

    g = fresh()
    snap = None
    for t in range(1, 100):
        g.turn = t
        p = g.players[g.current]
        g.begin_turn(p)
        if g.over:
            break
        if t == 3:
            snap = g.clone()
        g.agents[p.idx].take_turn(g, p)
        if g.over:
            break
        g.end_turn(p)
        g.current = 1 - g.current
    original = (g.winner, g.turn, g.over)
    if snap is None:
        pytest.skip("game ended before turn 3")

    c = snap
    for t in range(3, 100):
        c.turn = t
        p = c.players[c.current]
        if t > 3:
            c.begin_turn(p)
        if c.over:
            break
        c.agents[p.idx].take_turn(c, p)
        if c.over:
            break
        c.end_turn(p)
        c.current = 1 - c.current
    assert (c.winner, c.turn, c.over) == original
