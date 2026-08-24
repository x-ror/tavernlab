"""Regressions for four bugs found in review, all of the same family:
a signal that looked right on the synthetic fixtures and was wrong on a
real log.

12  a player's own TURN tag was counted as the game turn
13  the overlay never learned a hero power was already spent
14  a `partial` review carries a complete summary and was hidden anyway
15  `approx` never reached the classifier, so a bounded search shipped
    at 0.95 confidence with its warning suppressed
"""
import json
import logging
import os

import pytest

from capture.hslog_import import import_log, parsed_games
from eval import classify as cl
from eval.mapper import build_overlay
from eval.review import build_review, read_outcome
from eval.snapshots import build_snapshots
from eval.types import EntityView, VisibleState
from store import Store

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs",
                   "fixtures")
TAKEN = os.path.join(FIX, "synthetic_fireball.log")
REAL1 = os.path.join(FIX, "real_game1.log.gz")


@pytest.fixture(autouse=True)
def quiet_hslog():
    logging.disable(logging.WARNING)
    yield
    logging.disable(logging.NOTSET)


def tag(eid, name, value):
    return {"type": "TAG_CHANGE", "entity_id": eid, "tag": name,
            "value": value}


def create_game(game_eid=1):
    return {"type": "CREATE_GAME", "entity_id": game_eid, "tags": {},
            "players": [{"entity_id": 2, "player_id": 1, "tags": {}},
                        {"entity_id": 3, "player_id": 2, "tags": {}}]}


# ── 12 ────────────────────────────────────────────────────────────────
def test_only_the_game_entity_carries_the_turn_counter():
    """The real tail is `GameEntity TURN=13`, `Player TURN=7`, `WON`.
    Counting both put the kill on turn 7 while the decision sat in
    bucket 13 — a missed lethal on the turn you won with."""
    events = [create_game(),
              tag(1, "TURN", 13),
              tag(2, "TURN", 7),          # player 1's own counter
              tag(3, "TURN", 6),
              tag(2, "PLAYSTATE", "WON"),
              tag(3, "PLAYSTATE", "LOST")]
    out = read_outcome(events, us_pid=1)
    assert out["result"] == "win"
    assert out["end_turn"] == 13, "a player TURN tag leaked into the clock"
    assert out["lethal_turn"] == 13


def test_a_non_default_game_entity_id_still_works():
    events = [create_game(game_eid=4), tag(4, "TURN", 9), tag(2, "TURN", 5),
              tag(2, "PLAYSTATE", "WON")]
    assert read_outcome(events, us_pid=1)["end_turn"] == 9


@pytest.mark.parametrize("path", [TAKEN, REAL1])
def test_end_turn_matches_the_logs_own_turn_count(path):
    _raw, events, summ, _p = list(parsed_games(path))[0]
    out = read_outcome(events, us_pid=1)
    assert out["end_turn"] == summ.turns


def test_the_turn_you_won_on_is_not_a_missed_lethal(carddb, tmp_path):
    """End-to-end on the fixture that now reproduces the real tail."""
    st = Store(str(tmp_path / "t.sqlite"))
    try:
        gid = import_log(st, TAKEN)[0]
        game = dict(st.get_game(gid))
        events = [json.loads(r["payload"]) for r in st.get_events(gid)]
    finally:
        st.close()
    assert game["result"] == "win"

    review, decisions, _snaps = build_review(events, game)
    labels = {d["label"] for d in decisions if d["label"]}
    assert "missed_lethal" not in labels, \
        "flagged a missed lethal on the turn the game was won"
    assert labels == {"played_lethal"}
    assert not [m for m in review["key_moments"]
                if m["label"] == "missed_lethal"]
    assert "Thrown" not in review["report"]["headline"]


def test_a_win_marks_our_last_turn_taken_even_if_the_clock_slips(carddb):
    """Belt and braces: whatever order the closing tags arrive in, the
    last turn we acted on is the turn we killed on."""
    _raw, events, _s, _p = list(parsed_games(TAKEN))[0]
    game = {"id": 1, "player_id": 1, "player_class": "MAGE",
            "opponent_class": "ROGUE", "result": "win"}
    # Strip the game-wide TURN entirely: the clock now cannot help.
    stripped = [e for e in events
                if not (e.get("type") == "TAG_CHANGE"
                        and e.get("tag") == "TURN"
                        and e.get("entity_id") == 1)]
    _review, decisions, _snaps = build_review(stripped, game)
    assert "missed_lethal" not in {d["label"] for d in decisions}


# ── 13 ────────────────────────────────────────────────────────────────
def hero_power_state(exhausted, card_id="CS2_034", cost=2):
    vs = VisibleState(turn=7, us=1, current_player=1)
    vs.heroes = {1: {"hp": 30, "armor": 0, "atk": 0, "attacks": 0},
                 2: {"hp": 1, "armor": 0, "atk": 0, "attacks": 0}}
    vs.mana = {1: {"crystals": 8, "used": 0}, 2: {"crystals": 8}}
    vs.deck_counts = {1: 20, 2: 20}
    vs.boards = {1: [], 2: []}
    vs.hands = {1: [], 2: []}
    if card_id is not None:
        vs.hero_powers = {1: {"eid": 55, "card_id": card_id,
                              "name": None, "cost": cost,
                              "exhausted": exhausted}}
    return vs


def test_a_spent_hero_power_cannot_finish_the_game(carddb):
    """Fireblast on a 1-hp hero is lethal only if it has not been fired
    yet. This is the false positive the design calls product-ending."""
    from hs2.lethal import find_lethal

    free = build_overlay(hero_power_state(False), us_cls="MAGE",
                         them_cls="PRIEST")
    assert free.us.hero_power.used == 0
    assert find_lethal(free.game, free.us) is not None

    spent = build_overlay(hero_power_state(True), us_cls="MAGE",
                          them_cls="PRIEST")
    assert spent.us.hero_power.used >= spent.us.hero_power.uses_per_turn
    assert find_lethal(spent.game, spent.us) is None, \
        "counted a hero power the player had already used"


def test_a_replaced_hero_power_beats_the_class_default(carddb):
    """One real fixture has a Priest holding `Blessing of the Moon`; the
    class default would have been the wrong card entirely."""
    vs = hero_power_state(False, card_id="EDR_449p")
    ov = build_overlay(vs, us_cls="PRIEST", them_cls="HUNTER")
    assert ov.us.hero_power.card.id == "EDR_449p"
    assert ov.us.hero_power.card.name != "Lesser Heal"


def test_an_unknown_hero_power_is_treated_as_used(carddb):
    """A power can only ever add damage, so assuming it is spent costs a
    lethal at worst; assuming it is free invents one."""
    ov = build_overlay(hero_power_state(False, card_id=None),
                       us_cls="MAGE", them_cls="PRIEST")
    assert ov.us.hero_power.used >= ov.us.hero_power.uses_per_turn
    assert any("hero power state unknown" in r for r in ov.reasons)


def test_the_reducer_tracks_hero_power_exhaustion_on_a_real_log():
    _raw, events, _s, _p = list(parsed_games(REAL1))[0]
    states, _points = build_snapshots(events, 1)
    seen = {bool((s.hero_powers.get(1) or {}).get("exhausted"))
            for s in states}
    assert seen == {True, False}, \
        "exhaustion is either never or always set — the tag is not read"
    # …and the *card* changes when the power is replaced mid-game.
    cards = [(s.hero_powers.get(1) or {}).get("card_id") for s in states]
    assert len({c for c in cards if c}) > 1


# ── 15 ────────────────────────────────────────────────────────────────
def test_an_approximate_lethal_is_published_at_lower_confidence():
    base = {"lethal_ok": True, "available": True, "taken": False}
    exact = cl.classify(cl.gates_for(lethal=base))
    approx = cl.classify(cl.gates_for(lethal=dict(base, approx=True)))
    assert exact.label == approx.label == "missed_lethal"
    assert exact.conf == 0.95
    assert approx.conf == 0.75, \
        "a bounded taunt search shipped at full confidence"
    assert cl.gates_for(lethal=dict(base, approx=True))["approx"] is True


def test_approx_survives_the_whole_finding_round_trip(carddb):
    from eval.solvers.lethal import LethalFinding
    f = LethalFinding()
    f.available = f.lethal_ok = True
    f.approx = True
    assert cl.classify(cl.gates_for(lethal=f.to_dict())).conf == 0.75
    f.approx = False
    assert cl.classify(cl.gates_for(lethal=f.to_dict())).conf == 0.95
