"""PR 7: VisibleState reconstructor + decision-point snapshots.

Acceptance from the design: friendly HAND / PLAY and both heroes come
back on all three fixtures, the reconstruction is independent of `hs2`,
`search_ok` is 0 everywhere, and snapshots are taken at decision points
and turn boundaries — never one per TAG_CHANGE (§2.5 size budget).
"""
import ast
import json
import logging
import os
import sys

import pytest

from capture import hslog_import as imp
from eval import snapshots as sn
from eval import visible as vis
from eval.types import VisibleState
from store import Store

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs",
                   "fixtures")
REAL1 = os.path.join(FIX, "real_game1.log.gz")
REAL2 = os.path.join(FIX, "real_game2.log.gz")
SYNTH = os.path.join(FIX, "synthetic_fireball.log")
REAL = [REAL1, REAL2]
ALL = [REAL1, REAL2, SYNTH]


@pytest.fixture(autouse=True)
def quiet_hslog():
    """hslog logs `Broken option nesting` / `Orphaned BLOCK_END` for real
    logs and recovers; the noise would drown the test output."""
    logging.disable(logging.WARNING)
    yield
    logging.disable(logging.NOTSET)


def load(path):
    """`(events, us_pid)` for one fixture — one game slice each."""
    _raw, evs, summ, _p = list(imp.parsed_games(path))[0]
    return evs, imp._pick_us(summ, None, None)


# --------------------------------------------------------- reconstruction
@pytest.mark.parametrize("path", ALL)
def test_every_fixture_reconstructs_without_raising(path):
    evs, us = load(path)
    rec = vis.Reconstructor(us).run(evs)
    assert rec.errors == [], "the reducer must degrade, not blow up"
    assert rec.game_eid is not None
    assert set(rec.player_eid) == {1, 2}
    final = rec.state()
    assert final.seq == len(evs) - 1
    assert final.us == us


@pytest.mark.parametrize("path", ALL)
def test_visible_states_round_trip_through_json(path):
    """`to_dict`/`from_dict` must be a fixed point: `EntityView.to_dict`
    drops falsy fields, so the stable identity is over the *dict*."""
    evs, us = load(path)
    states, _pts = sn.build_snapshots(evs, us)
    assert states
    for state in states:
        d = state.to_dict()
        back = VisibleState.from_dict(json.loads(json.dumps(d)))
        assert back.to_dict() == d
        assert back.turn == state.turn and back.us == state.us
        assert [e.eid for e in back.board(us)] == \
               [e.eid for e in state.board(us)]


# ------------------------------------------------------ synthetic fixture
def test_synthetic_fireball_before_and_after_the_play_block():
    """The design's annotated excerpt, reduced: Fireball in our hand and
    a 2/3 taunt opposite; after the block neither is where it was."""
    evs, _us = load(SYNTH)
    states, points = sn.build_snapshots(evs, 1)
    i = next(i for i, p in enumerate(points) if p["kind"] == "play")
    assert points[i]["entity_id"] == 64 and points[i]["target_id"] == 66
    assert points[i]["card_id"] == "CORE_CS2_029"

    before = states[i]
    assert before.turn == 7
    hand = {e.eid: e for e in before.hand(1)}
    assert 64 in hand
    assert hand[64].card_id == "CORE_CS2_029"
    assert hand[64].name == "Fireball"
    assert hand[64].zone == "HAND"
    board = {e.eid: e for e in before.board(2)}
    assert 66 in board
    assert board[66].has("TAUNT")
    assert (board[66].atk, board[66].health) == (2, 3)

    end = next(j for j in range(points[i]["seq"], len(evs))
               if evs[j]["type"] == "BLOCK_END"
               and evs[j].get("block_type") == "PLAY")
    after = vis.Reconstructor(1).run(evs[:end + 1]).state()
    assert 64 not in {e.eid for e in after.hand(1)}
    assert 66 not in {e.eid for e in after.board(2)}
    assert after.turn == 7


# ----------------------------------------------------- friendly checkpoint
@pytest.mark.parametrize("path", REAL)
def test_real_fixture_hand_board_and_hero_checkpoints(path):
    evs, us = load(path)
    states, _pts = sn.build_snapshots(evs, us)
    assert len(states) > 10
    them = 2 if us == 1 else 1
    for state in states:
        for pid in (us, them):
            assert 0 <= len(state.hand(pid)) <= 10, state.seq
            assert 0 <= len(state.board(pid)) <= 7, state.seq
            slots = [e.zone_pos for e in state.board(pid) if e.zone_pos]
            assert len(set(slots)) == len(slots), "duplicate board slot"
            assert state.hero(pid), "hero missing from the projection"
            assert state.hp_total(pid) >= 0, (state.seq, pid)
            assert state.deck_counts[pid] >= 0
    for pid in (us, them):
        assert states[0].hero(pid)["hp"] == 30, "hero does not start at 30"
    turns = [s.turn for s in states]
    assert turns == sorted(turns), "turn numbers went backwards"
    assert max(turns) > 1


# -------------------------------------------------------- decision points
@pytest.mark.parametrize("path", REAL)
def test_decision_points_are_plausible_and_sparse(path):
    evs, _us = load(path)
    points = sn.decision_points(evs)
    kinds = [p["kind"] for p in points]
    actions = [k for k in kinds if k in ("play", "attack")]
    assert len(actions) > 10, kinds
    assert "turn_start" in kinds and "mulligan" in kinds
    for p in points:
        assert 0 <= p["seq"] < len(evs)
        assert p["side"] in ("us", "them")
        assert p["turn"] >= 0
    seqs = [p["seq"] for p in points]
    assert seqs == sorted(set(seqs)), "points must be ordered and unique"

    # The size budget: decision points + turn boundaries, never one row
    # per TAG_CHANGE (design §2.5).
    states, _pts = sn.build_snapshots(evs, 1)
    assert len(states) == len(points)
    assert len(states) < len(evs) / 10, "far too many snapshots"


def test_a_play_point_names_the_card_that_was_played():
    evs, _us = load(REAL1)
    points = sn.decision_points(evs)
    plays = [p for p in points if p["kind"] == "play" and p["card_id"]]
    assert plays, "no play point resolved a card id"
    for p in plays:
        assert evs[p["seq"]]["type"] == "BLOCK_START"
        assert evs[p["seq"]]["block_type"] == "PLAY"
        assert evs[p["seq"]]["entity_id"] == p["entity_id"]


def test_mulligan_point_carries_the_offer_and_what_was_kept():
    evs, _us = load(REAL1)
    mull = [p for p in sn.decision_points(evs) if p["kind"] == "mulligan"]
    assert len(mull) == 1
    choices = mull[0]["choices"]
    assert choices, "mulligan offer lost"
    assert all(set(c) == {"eid", "card_id", "picked"} for c in choices)
    assert any(c["picked"] for c in choices)


# ------------------------------------------- the reconstruction is sound
# `capture.events.walk()` recurses into any packet with children, not
# only `Block`: hslog nests packets under `SubSpell` too, and a minion
# that bounces itself to hand inside a spell effect leaves play in
# exactly those children.  These pin the consequence from this side, so
# a regression there fails the replay suite and not only the importer's.
@pytest.mark.parametrize("path", REAL)
def test_board_invariants_hold_without_the_repair(path, monkeypatch):
    """Bypass `_repair_board` completely: the raw fold must already obey
    Hearthstone's board rules at every snapshot."""
    monkeypatch.setattr(vis, "_repair_board", lambda ents: list(ents))
    evs, us = load(path)
    states, _pts = sn.build_snapshots(evs, us)
    assert states
    for state in states:
        for pid in (1, 2):
            board = state.board(pid)
            assert len(board) <= 7, (state.seq, pid, len(board))
            assert len(state.minions(pid)) <= 7, (state.seq, pid)
            slots = [e.zone_pos for e in board if e.zone_pos]
            assert len(set(slots)) == len(slots), (state.seq, pid, slots)


@pytest.mark.parametrize("path", ALL)
def test_the_board_repair_never_has_to_fire(path, monkeypatch):
    """A firing means the fold lost a card's exit from play."""
    fired = []
    original = vis._repair_board

    def watched(ents):
        out = original(ents)
        if len(out) != len(ents):
            fired.append(([e.eid for e in ents], [e.eid for e in out]))
        return out

    monkeypatch.setattr(vis, "_repair_board", watched)
    evs, us = load(path)
    sn.build_snapshots(evs, us)
    assert fired == [], "the guard fired: an exit from play went missing"


@pytest.mark.parametrize("path", REAL)
def test_hero_hp_never_needs_the_clamp_at_a_decision_point(path):
    """`hp` is clamped at 0 because a hero is dead at 0, but the raw
    HEALTH/DAMAGE pair must already be non-negative anywhere a review
    looks — the clamp only shapes the post-lethal epilogue."""
    evs, us = load(path)
    states, _pts = sn.build_snapshots(evs, us)
    for state in states:
        for pid in (1, 2):
            hero = state.hero(pid)
            raw = hero["max_hp"] - hero["damage"]
            assert raw >= 0, (state.seq, pid, hero)
            assert hero["hp"] == raw, "the clamp is not inert here"


def test_a_minion_bounced_inside_a_sub_spell_leaves_the_board():
    """Entity 32 (Escape Artist) returns itself to hand inside a
    `SUB_SPELL`.  While those children were dropped it sat on the
    opponent's board for the rest of the game."""
    evs, _us = load(REAL2)
    states, _pts = sn.build_snapshots(evs, 1)
    on = [s.seq for s in states if 32 in {e.eid for e in s.board(2)}]
    assert on, "entity 32 never reached the board — did the fixture move?"
    assert len(on) < len(states) / 4, "the bounced minion is stuck in play"
    assert on == sorted(on)

    rec = vis.Reconstructor(1)
    zones = []
    for seq, ev in enumerate(evs):
        rec.apply(ev, seq)
        e = rec.entities.get(32)
        if e is not None and (not zones or zones[-1][1] != e.zone()):
            zones.append((seq, e.zone()))
    assert [z for _s, z in zones] == ["DECK", "HAND", "PLAY", "SETASIDE"]
    left_at = zones[-1][0]
    assert max(on) < left_at, "on the board after it bounced"
    # The `HideEntity` that rides along with the bounce hides the card
    # again, so the eid survives with no card id.
    assert rec.entities[32].card_id is None
    assert 32 not in {e.eid for e in states[-1].board(2)}


def test_sub_spell_markers_are_no_ops_for_state():
    """`walk()` brackets a sub-spell's children with a balanced
    start/end marker.  Neither carries state."""
    rec = vis.Reconstructor(1)
    rec.apply({"type": "CREATE_GAME", "entity_id": 1, "tags": {"TURN": 4},
               "players": [{"entity_id": 2, "player_id": 1, "tags": {}},
                           {"entity_id": 3, "player_id": 2, "tags": {}}]})
    before = rec.state().to_dict()
    rec.apply({"type": "SUB_SPELL", "phase": "start", "prefab": "FX",
               "source": 7, "target_count": 1, "targets": [9]})
    rec.apply({"type": "SUB_SPELL", "phase": "end", "prefab": "FX",
               "source": 7})
    after = rec.state().to_dict()
    assert rec.depth == 0, "a sub-spell must not open a block"
    assert {k: v for k, v in after.items() if k != "seq"} == \
           {k: v for k, v in before.items() if k != "seq"}
    assert rec.errors == []


# --------------------------------------------------------- MVP invariants
@pytest.mark.parametrize("path", ALL)
def test_search_ok_is_false_on_every_snapshot(path):
    """Design §2.6: the trigger graph is never reconstructed in MVP, so
    no snapshot may claim it was."""
    evs, us = load(path)
    states, points = sn.build_snapshots(evs, us)
    assert all(s.search_ok is False for s in states)
    assert all(s.lethal_ok is False for s in states), "PR 8 owns lethal_ok"
    rows = sn.snapshot_rows(states, points)
    assert all(r["search_ok"] == 0 for r in rows)
    assert all(r["wp"] is None and r["wp_source"] is None for r in rows)


def test_visible_module_never_imports_hs2_at_module_level():
    """`eval.visible` has to reconstruct a game whose cards `hs2` cannot
    simulate — including when `hs2` will not import at all."""
    src = open(vis.__file__, encoding="utf-8").read()
    tree = ast.parse(src)
    top = []
    for node in tree.body:
        if isinstance(node, ast.Import):
            top += [a.name for a in node.names]
        elif isinstance(node, ast.ImportFrom):
            top.append(node.module or "")
    assert not [m for m in top if m.split(".")[0] == "hs2"], top


def test_reconstruction_survives_hs2_being_unimportable(monkeypatch):
    """The whole point of §2.6: a game whose cards `hs2` cannot simulate
    still reconstructs and still gets reviewed at `search_ok=0`."""
    monkeypatch.setitem(sys.modules, "hs2", None)
    monkeypatch.setitem(sys.modules, "hs2.carddata", None)
    with pytest.raises(ImportError):
        import hs2.carddata                      # noqa: F401 - the probe
    evs, _us = load(SYNTH)
    states, _pts = sn.build_snapshots(evs, 1)
    assert states[1].implemented_gap == [], "gap must degrade to empty"
    # Names come from the HSJSON data file, not from the engine.
    assert [e.name for e in states[1].hand(1)] == ["Fireball"]


def test_implemented_gap_lists_only_unimplemented_visible_cards(carddb):
    assert vis.implemented_gap([]) == []
    assert vis.implemented_gap([None]) == []
    assert vis.implemented_gap(["NO_SUCH_CARD_9999"]) == ["NO_SUCH_CARD_9999"]
    # Fireball is implemented, so a hand holding it reports no gap.
    assert vis.implemented_gap(["CORE_CS2_029"]) == []


def test_card_names_come_from_the_hsjson_corpus():
    assert vis.card_name("CORE_CS2_029") == "Fireball"
    assert vis.card_name("NO_SUCH_CARD_9999") is None
    assert vis.card_name(None) is None


# -------------------------------------------------------------- tolerance
def test_reducer_degrades_on_broken_and_unknown_input():
    rec = vis.Reconstructor(1)
    rec.apply({"type": "CREATE_GAME", "entity_id": 1,
               "tags": {"TURN": 3},
               "players": [{"entity_id": 2, "player_id": 1, "tags": {}},
                           {"entity_id": 3, "player_id": 2, "tags": {}}]})
    # BLOCK_END with no start must not push the depth negative, or every
    # later block would look top level.
    rec.apply({"type": "BLOCK_END", "block_type": "PLAY", "entity_id": 9})
    assert rec.depth == 0
    # An entity nobody created, and a tag nobody has heard of.
    rec.apply({"type": "TAG_CHANGE", "entity_id": 999, "tag": "ZONE",
               "value": "PLAY"})
    rec.apply({"type": "TAG_CHANGE", "entity_id": 999, "tag": "CARDTYPE",
               "value": "MINION"})
    rec.apply({"type": "TAG_CHANGE", "entity_id": 999, "tag": "CONTROLLER",
               "value": 2})
    rec.apply({"type": "TAG_CHANGE", "entity_id": 999, "tag": "4242",
               "value": 7})
    rec.apply({"type": "TAG_CHANGE", "entity_id": 999, "tag": "TAUNT",
               "value": 1})
    # Events with no state, a missing field, and outright garbage.
    for ev in ({"type": "META_DATA", "meta": "DAMAGE", "data": [3]},
               {"type": "OPTIONS", "id": 1},
               {"type": "SUB_SPELL"}, {"type": "SHUFFLE_DECK"},
               {"type": "SEND_OPTION"}, {"type": "CHOSEN_ENTITIES"},
               {"type": "OTHER", "packet": "Whatever"},
               {"type": "TAG_CHANGE"}, {"type": "FULL_ENTITY"},
               {"type": "NEVER_HEARD_OF_IT"}, {}, None, 42):
        rec.apply(ev)
    state = rec.state()
    assert state.turn == 3
    minion = {e.eid: e for e in state.board(2)}[999]
    assert minion.has("TAUNT")
    assert minion.tags["4242"] == 7, "unknown tags are kept verbatim"
    assert minion.card_id is None and minion.name is None
    assert rec.errors == [] or all(e[1] is None for e in rec.errors)


def test_hide_entity_clears_the_card_id_but_keeps_the_entity():
    rec = vis.Reconstructor(1)
    rec.apply({"type": "CREATE_GAME", "entity_id": 1, "tags": {},
               "players": [{"entity_id": 2, "player_id": 1, "tags": {}},
                           {"entity_id": 3, "player_id": 2, "tags": {}}]})
    rec.apply({"type": "FULL_ENTITY", "entity_id": 7,
               "card_id": "CORE_CS2_029",
               "tags": {"ZONE": "HAND", "CONTROLLER": 1,
                        "CARDTYPE": "SPELL", "COST": 4}})
    hand = {e.eid: e for e in rec.state().hand(1)}
    assert hand[7].card_id == "CORE_CS2_029" and hand[7].cost == 4
    rec.apply({"type": "HIDE_ENTITY", "entity_id": 7, "zone": "HAND"})
    hand = {e.eid: e for e in rec.state().hand(1)}
    assert 7 in hand, "a hidden card is still a card in hand"
    assert hand[7].card_id is None and hand[7].name is None


def test_change_entity_transforms_the_card_id():
    rec = vis.Reconstructor(1)
    rec.apply({"type": "CREATE_GAME", "entity_id": 1, "tags": {},
               "players": [{"entity_id": 2, "player_id": 1, "tags": {}},
                           {"entity_id": 3, "player_id": 2, "tags": {}}]})
    rec.apply({"type": "FULL_ENTITY", "entity_id": 7, "card_id": "CS2_231",
               "tags": {"ZONE": "PLAY", "CONTROLLER": 1,
                        "CARDTYPE": "MINION", "ATK": 1, "HEALTH": 1}})
    rec.apply({"type": "CHANGE_ENTITY", "entity_id": 7,
               "card_id": "CORE_CS2_029", "tags": {"CARDTYPE": "SPELL"}})
    assert rec.entities[7].card_id == "CORE_CS2_029"


def test_create_game_resets_a_dirty_entity_map():
    evs, _us = load(SYNTH)
    rec = vis.Reconstructor(1).run(evs)
    assert rec.entities
    rec.apply(evs[0], 500)                  # a second CREATE_GAME
    assert set(rec.entities) == {1, 2, 3}, "stale entities survived"
    assert rec.state().board(2) == []
    assert rec.seq == 500, "the fold's position is not game state"


def test_state_is_a_deep_copy_callers_may_mutate():
    evs, _us = load(SYNTH)
    rec = vis.Reconstructor(1).run(evs[:7])
    first = rec.state()
    first.hand(1)[0].card_id = "MUTATED"
    first.hand(1).clear()
    first.heroes[1]["hp"] = -99
    second = rec.state()
    assert [e.card_id for e in second.hand(1)] == ["CORE_CS2_029"]
    assert second.heroes[1]["hp"] == 30


# ------------------------------------------------------------------ store
def test_snapshots_write_and_read_back_from_the_store(tmp_path):
    store = Store(str(tmp_path / "t.sqlite"))
    try:
        gid = imp.import_log(store, SYNTH, player_name="Player1#00001")[0]
        evs = [json.loads(r["payload"]) for r in store.get_events(gid)]
        states, points = sn.build_snapshots(evs, store.get_game(gid)
                                            ["player_id"])
        rows = sn.snapshot_rows(states, points)
        n = store.submit("add_snapshots", gid, imp.PARSE_GENERATION, rows)
        assert n == len(rows)

        back = store.get_snapshots(gid)
        assert [r["event_seq"] for r in back] == [s.seq for s in states]
        assert all(r["parse_generation"] == imp.PARSE_GENERATION
                   for r in back)
        assert all(r["search_ok"] == 0 for r in back)
        assert all(r["wp"] is None for r in back)
        restored = [VisibleState.from_dict(json.loads(r["visible"]))
                    for r in back]
        assert [s.to_dict() for s in restored] == \
               [s.to_dict() for s in states]
        assert json.loads(back[0]["unimplemented"]) == \
            states[0].implemented_gap
    finally:
        store.close()


def test_store_reads_only_the_newest_finished_generation(tmp_path):
    """Generation rule (§2.2): a gen-0 live tail must never be mixed in
    with the gen-1 hslog reparse the snapshots belong to."""
    store = Store(str(tmp_path / "t.sqlite"))
    try:
        gid = imp.import_log(store, SYNTH)[0]
        evs = [json.loads(r["payload"]) for r in store.get_events(gid)]
        states, points = sn.build_snapshots(evs, 1)
        store.submit("add_snapshots", gid, 0,
                     sn.snapshot_rows(states[:1], points[:1]))
        store.submit("add_snapshots", gid, imp.PARSE_GENERATION,
                     sn.snapshot_rows(states, points))
        back = store.get_snapshots(gid)
        assert len(back) == len(states)
        assert {r["parse_generation"] for r in back} == \
               {imp.PARSE_GENERATION}
    finally:
        store.close()
