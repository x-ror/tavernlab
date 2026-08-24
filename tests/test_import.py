"""PR 5: canonical packet importer.

Acceptance from the design: 3/3 fixtures yield winner + classes + turn
count, and a Fireball PLAY block's walked children include the inner
`TAG_CHANGE ZONE` and the `META_DATA DAMAGE` — the assertion a
non-recursive `for p in tree.packets` cannot pass.
"""
import gzip
import json
import logging
import os

import pytest

from capture import events as ce
from capture import hslog_import as imp
from store import Store

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs",
                   "fixtures")
REAL1 = os.path.join(FIX, "real_game1.log.gz")
REAL2 = os.path.join(FIX, "real_game2.log.gz")
SYNTH = os.path.join(FIX, "synthetic_fireball.log")
ALL = [REAL1, REAL2, SYNTH]


@pytest.fixture(autouse=True)
def quiet_hslog():
    """hslog logs `Broken option nesting` / `Orphaned BLOCK_END` for real
    logs and recovers; the noise would drown the test output."""
    logging.disable(logging.WARNING)
    yield
    logging.disable(logging.NOTSET)


def parse(path):
    return list(imp.parsed_games(path))


# ------------------------------------------------------------- acceptance
@pytest.mark.parametrize("path", ALL)
def test_fixture_yields_winner_classes_and_turns(path):
    games = parse(path)
    assert len(games) == 1, "each fixture is exactly one game slice"
    _raw, evs, summ, _p = games[0]
    assert summ.winner_pid in (1, 2), "no winner extracted"
    assert set(summ.players) == {1, 2}
    assert set(summ.classes) == {1, 2}, summ.classes
    for cls in summ.classes.values():
        assert cls.isupper() and cls.isalpha()
    assert summ.turns > 0
    assert summ.result_for(summ.winner_pid) == "win"
    other = 2 if summ.winner_pid == 1 else 1
    assert summ.result_for(other) == "loss"
    assert evs and evs[0]["type"] == ce.CREATE_GAME


def test_play_block_keeps_its_inner_zone_and_metadata():
    """The design's PR 5 gate. A flat iterate over `tree.packets` drops
    every TAG_CHANGE / META_DATA nested in a PLAY block."""
    _raw, evs, _s, _p = parse(SYNTH)[0]
    i = next(i for i, e in enumerate(evs)
             if e["type"] == ce.BLOCK_START and e["block_type"] == "PLAY")
    depth, inner = 1, []
    for e in evs[i + 1:]:
        if e["type"] == ce.BLOCK_START:
            depth += 1
        elif e["type"] == ce.BLOCK_END:
            depth -= 1
            if depth == 0:
                break
        inner.append(e)
    assert evs[i]["entity_id"] == 64            # Fireball
    assert evs[i]["target_id"] == 66            # the taunt
    assert any(e["type"] == ce.TAG_CHANGE and e["tag"] == "ZONE"
               and e["entity_id"] == 64 for e in inner)
    dmg = [e for e in inner if e["type"] == ce.META_DATA
           and e["meta"] == "DAMAGE"]
    assert dmg and dmg[0]["data"] == [6] and dmg[0]["info"] == [66]
    assert any(e["type"] == ce.BLOCK_START and e["block_type"] == "DEATHS"
               for e in inner), "nested block lost"


@pytest.mark.parametrize("path", ALL)
def test_blocks_are_balanced_and_types_are_closed(path):
    _raw, evs, _s, _p = parse(path)[0]
    depth = 0
    for e in evs:
        assert e["type"] in ce.EVENT_TYPES, e["type"]
        assert json.dumps(e), "event payload must be JSON-serializable"
        if e["type"] == ce.BLOCK_START:
            depth += 1
        elif e["type"] == ce.BLOCK_END:
            depth -= 1
            assert depth >= 0, "BLOCK_END without BLOCK_START"
    assert depth == 0, "unclosed block"


def test_unknown_packets_become_other_and_do_not_crash():
    """Packets outside the closed catalog (hslog's
    `CachedTagForDormantChange`, or anything a newer hslog adds) must be
    stored as OTHER, not crash the import."""
    from hslog.packets import CachedTagForDormantChange

    p = CachedTagForDormantChange(ts="12:00:00", entity=95, tag=45, value=5)
    ev = ce.canonicalize(p)
    assert ev["type"] == ce.OTHER
    assert ev["packet"] == "CachedTagForDormantChange"
    assert ev["raw"]["entity"] == 95 and ev["raw"]["value"] == 5
    assert json.dumps(ev)

    class FutureHslogPacket:
        def __init__(self):
            self.ts = "12:00:00"
            self.entity = 7
            self.brand_new_field = "whatever"

    ev = ce.canonicalize(FutureHslogPacket())
    assert ev["type"] == ce.OTHER
    assert ev["packet"] == "FutureHslogPacket"
    assert ev["raw"]["brand_new_field"] == "whatever"
    assert ce.walk([FutureHslogPacket()], []) == [dict(ev, ts="12:00:00")]


# ---------------------------------------------------------------- streams
def test_one_logger_stream_never_both():
    lines = imp.read_lines(REAL1)
    assert imp.choose_logger(lines) == "PowerTaskList", \
        "PowerTaskList is preferred when it emitted anything"
    kept = imp.filter_logger(lines, "PowerTaskList")
    power = [ln for ln in kept if "DebugPrintPower()" in ln]
    assert power, "no power lines survived the filter"
    assert all("PowerTaskList." in ln for ln in power), \
        "both power streams kept: the tree would be doubled"
    # GameState side channels must survive: only GameState logs choices,
    # options and game meta.
    assert any("DebugPrintGame()" in ln for ln in kept)


def test_slices_split_on_the_boundary_logger():
    lines = imp.read_lines(REAL1)
    assert imp.boundary_logger(lines) == "GameState"
    assert len(imp.split_games(lines, "GameState")) == 1


def test_gzip_and_plain_fixtures_read_the_same_way():
    plain = imp.read_lines(SYNTH)
    assert plain and plain[0].startswith("D ")
    packed = os.path.join(FIX, "synthetic_fireball.log")
    assert imp.read_lines(packed) == plain


# --------------------------------------------------------------- identity
def test_friendly_controller_is_none_without_zone_log(tmp_path):
    assert imp.friendly_controller(None) is None
    assert imp.friendly_controller(str(tmp_path / "nope.log")) is None


def test_friendly_controller_reads_zone_log(tmp_path):
    z = tmp_path / "Zone.log"
    z.write_text(
        "D 1 ZoneChangeList.ProcessChanges() - id=1 local=False "
        "[entityName=x id=4 zone=DECK zonePos=0 cardId= player=2] "
        "zone from  -> FRIENDLY DECK\n"
        "D 1 ZoneChangeList.ProcessChanges() - id=1 local=False "
        "[entityName=x id=9 zone=DECK zonePos=0 cardId= player=1] "
        "zone from  -> OPPOSING DECK\n")
    assert imp.friendly_controller(str(z)) == 2


def test_pick_us_prefers_zone_log_then_battletag():
    _raw, _e, summ, _p = parse(SYNTH)[0]
    assert imp._pick_us(summ, 2, None) == 2
    assert imp._pick_us(summ, None, "Player2#00002") == 2
    assert imp._pick_us(summ, None, None) == 1     # floor


# ------------------------------------------------------------------ store
def test_import_writes_game_events_and_pending_review(tmp_path):
    store = Store(str(tmp_path / "t.sqlite"))
    try:
        ids = imp.import_log(store, SYNTH, player_name="Player1#00001")
        assert len(ids) == 1
        gid = ids[0]
        game = store.get_game(gid)
        assert game["player_class"] == "MAGE"
        assert game["opponent_class"] == "ROGUE"
        assert game["result"] == "win"
        assert game["turns"] == 7
        assert game["mode"] == "ranked_standard"
        assert game["format"] == "standard"
        assert game["going_first"] == 1
        assert game["log_hash"]
        assert gzip.decompress(game["raw_power"]).decode().startswith("D ")

        evs = store.get_events(gid)
        assert len(evs) == 23
        assert [e["seq"] for e in evs] == list(range(23))
        assert evs[0]["type"] == "CREATE_GAME"
        assert json.loads(evs[0]["payload"])["type"] == "CREATE_GAME"
        assert all(e["parse_generation"] == 1 for e in evs)

        assert store.get_review(gid)["status"] == "pending"
        assert store.pending_reviews() == [gid]
    finally:
        store.close()


def test_import_is_idempotent_per_slice(tmp_path):
    store = Store(str(tmp_path / "t.sqlite"))
    try:
        first = imp.import_log(store, SYNTH)
        again = imp.import_log(store, SYNTH)
        assert len(first) == 1 and again == []
        assert len(store.list_games()) == 1
    finally:
        store.close()


def test_import_real_fixture_end_to_end(tmp_path):
    store = Store(str(tmp_path / "t.sqlite"))
    try:
        ids = imp.import_log(store, REAL1)
        assert len(ids) == 1
        game = store.get_game(ids[0])
        assert game["result"] in ("win", "loss")
        assert game["player_class"] and game["opponent_class"]
        assert game["turns"] >= 10
        evs = store.get_events(ids[0])
        assert len(evs) > 2000
        kinds = {e["type"] for e in evs}
        assert {"BLOCK_START", "BLOCK_END", "TAG_CHANGE",
                "META_DATA"} <= kinds
    finally:
        store.close()


def test_newest_power_log_skips_tiny_files(tmp_path):
    small = tmp_path / "s1"
    small.mkdir()
    (small / "Power.log").write_text("D 1 GameState.DebugPrintPower() - x\n")
    big = tmp_path / "s2"
    big.mkdir()
    (big / "Power.log").write_text("x" * (imp.MIN_POWER_BYTES + 1))
    assert imp.newest_power_log(str(tmp_path)) == str(big / "Power.log")
    assert imp.newest_power_log(str(tmp_path / "missing")) is None


# ------------------------------------------------------------ containers
@pytest.mark.parametrize("path", [REAL1, REAL2])
def test_sub_spell_children_are_not_dropped(path):
    """`Block` is not the only container: hslog's `SubSpell` also has a
    `.packets` list, and a spell's real work often happens in there. The
    census below is against the raw packet tree, so a regression in
    `walk()` shows up as missing events rather than as a subtle
    reconstruction bug 40 turns later."""
    from collections import Counter
    from capture.hslog_import import (boundary_logger, choose_logger,
                                      filter_logger, parse_slice,
                                      read_lines, split_games)
    lines = read_lines(path)
    lg = choose_logger(lines)
    raw = split_games(lines, boundary_logger(lines))[0]
    tree, _p = parse_slice(filter_logger(raw, lg), lg)

    census = Counter()

    def count(packets):
        for p in packets:
            census[type(p).__name__] += 1
            kids = getattr(p, "packets", None)
            if kids:
                count(kids)

    count(tree.packets)
    _raw, evs, _s, _p = parse(path)[0]
    got = Counter(e["type"] for e in evs)
    assert got[ce.TAG_CHANGE] == census["TagChange"]
    assert got[ce.FULL_ENTITY] == census["FullEntity"]
    assert got[ce.HIDE_ENTITY] == census["HideEntity"]
    assert got[ce.META_DATA] == census["MetaData"]
    assert got[ce.BLOCK_START] == got[ce.BLOCK_END] == census["Block"]


@pytest.mark.parametrize("path", [REAL1, REAL2, SYNTH])
def test_sub_spell_markers_are_balanced(path):
    """Including childless sub-spells — an unclosed start would make a
    bracketing consumer swallow the rest of the stream."""
    _raw, evs, _s, _p = parse(path)[0]
    starts = [e for e in evs if e["type"] == ce.SUB_SPELL
              and e.get("phase") == "start"]
    ends = [e for e in evs if e["type"] == ce.SUB_SPELL
            and e.get("phase") == "end"]
    assert len(starts) == len(ends)
    depth = 0
    for e in evs:
        if e["type"] != ce.SUB_SPELL:
            continue
        depth += 1 if e.get("phase") == "start" else -1
        assert depth >= 0, "SUB_SPELL end before its start"
    assert depth == 0
