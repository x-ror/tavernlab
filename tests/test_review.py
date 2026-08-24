"""PR 10 end-to-end: Power.log -> events -> VisibleState -> Review JSON.

The fixtures are chosen to pin both directions of the MVP's only skill
label: `synthetic_missed_lethal.log` is a lethal that was on the board and
was thrown away, `synthetic_fireball.log` is the same board played
correctly.  The two real games are the negative control — a reviewer that
invents a missed lethal in a normal game is worse than useless.
"""
import json
import logging
import os

import pytest

from capture.hslog_import import import_log, parsed_games
from eval import classify as cl
from eval.review import build_review, read_outcome
from store import Store

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs",
                   "fixtures")
MISSED = os.path.join(FIX, "synthetic_missed_lethal.log")
TAKEN = os.path.join(FIX, "synthetic_fireball.log")
REAL1 = os.path.join(FIX, "real_game1.log.gz")
REAL2 = os.path.join(FIX, "real_game2.log.gz")
REAL = [REAL1, REAL2]
ALL = [MISSED, TAKEN, REAL1, REAL2]


@pytest.fixture(autouse=True)
def quiet_hslog():
    logging.disable(logging.WARNING)
    yield
    logging.disable(logging.NOTSET)


@pytest.fixture(scope="module")
def reviews(carddb):
    """Import every fixture once and review it; module-scoped because
    `real_game2` is 12k events."""
    import tempfile
    out = {}
    with tempfile.TemporaryDirectory() as tmp:
        st = Store(os.path.join(tmp, "t.sqlite"))
        try:
            for path in ALL:
                gid = import_log(st, path)[0]
                game = dict(st.get_game(gid))
                events = [json.loads(r["payload"])
                          for r in st.get_events(gid)]
                review, decisions, snaps = build_review(events, game)
                out[path] = (game, events, review, decisions, snaps)
        finally:
            st.close()
    return out


# ------------------------------------------------------------- the label
def test_missed_lethal_is_caught(reviews):
    game, _e, review, decisions, _s = reviews[MISSED]
    assert game["result"] == "loss"
    labelled = [d for d in decisions if d["label"] == "missed_lethal"]
    assert len(labelled) == 1, \
        "the label belongs to one ply, not to every ply of the turn"
    d = labelled[0]
    assert d["turn"] == 7 and d["side"] == "us"
    assert d["lethal_ok"] == 1 and d["lethal_available"] == 1
    assert "Fireball" in d["lethal_plan"]
    assert d["label_conf"] >= 0.9
    assert "Thrown on turn 7" in review["report"]["headline"]
    moments = [m for m in review["key_moments"]
               if m["label"] == "missed_lethal"]
    assert len(moments) == 1 and moments[0]["turn"] == 7
    assert moments[0]["seq"] == d["event_seq"]


def test_taken_lethal_is_a_quiet_ack_not_an_accusation(reviews):
    _g, _e, review, decisions, _s = reviews[TAKEN]
    labels = {d["label"] for d in decisions if d["label"]}
    assert labels == {"played_lethal"}
    assert "Thrown" not in review["report"]["headline"]
    assert not [m for m in review["key_moments"]
                if m["label"] == "missed_lethal"]


@pytest.mark.parametrize("path", REAL)
def test_real_games_do_not_invent_a_missed_lethal(reviews, path):
    _g, _e, review, decisions, _s = reviews[path]
    bogus = [d for d in decisions if d["label"] == "missed_lethal"]
    assert not bogus, [d["lethal_plan"] for d in bogus]


# -------------------------------------------------------------- honesty
@pytest.mark.parametrize("path", ALL)
def test_every_wp_point_is_hatched_logistic(reviews, path):
    _g, _e, review, _d, _s = reviews[path]
    assert review["wp_series"], "no win-probability series"
    for pt in review["wp_series"]:
        assert pt["source"] == "logistic_v1"
        assert pt["hatch"] is True, "an unhatched logistic point shipped"
        assert 0.0 <= pt["wp"] <= 1.0


@pytest.mark.parametrize("path", ALL)
def test_search_ok_and_actions_complete_are_off_everywhere(reviews, path):
    _g, _e, review, decisions, _s = reviews[path]
    for d in decisions:
        assert d["search_ok"] == 0
        assert d["actions_complete"] == 0
        assert d["search_depth"] == 0
    for turn in review["turns"]:
        for dec in turn["decisions"]:
            assert dec["search_ok"] is False
            assert dec["actions_complete"] is False


@pytest.mark.parametrize("path", ALL)
def test_delta_wp_is_never_used_to_rank_or_label(reviews, path):
    _g, _e, _r, decisions, _s = reviews[path]
    for d in decisions:
        assert d["delta_wp"] is None
        if d["label"]:
            assert d["label"] in cl.MVP_LABELS


@pytest.mark.parametrize("path", ALL)
def test_no_chesscom_glyph_ships(reviews, path):
    _g, _e, review, decisions, _s = reviews[path]
    banned = {"blunder", "mistake", "inaccuracy", "best", "brilliant",
              "lucky", "unlucky"}
    assert not banned & {d["label"] for d in decisions}
    assert not banned & {m["label"] for m in review["key_moments"]}
    legend = {e["key"]: e for e in review["labels_legend"]}
    for key in banned:
        assert legend[key]["available"] is False


@pytest.mark.parametrize("path", ALL)
def test_report_always_carries_the_wp_caveat(reviews, path):
    _g, _e, review, _d, _s = reviews[path]
    caveats = " ".join(review["report"]["caveats"])
    assert "9 weights" in caveats and "hatched" in caveats
    assert "search_ok=0" in caveats


def test_hidden_labels_explain_themselves(reviews):
    _g, _e, review, _d, _s = reviews[REAL1]
    unlabelled = [dec for turn in review["turns"]
                  for dec in turn["decisions"] if dec["label"] is None]
    assert unlabelled
    assert all(dec["label_reason"] for dec in unlabelled)


# ------------------------------------------------------- key moments etc
@pytest.mark.parametrize("path", REAL)
def test_key_moments_on_a_loss_are_ledger_notes_not_top_delta_wp(
        reviews, path):
    game, _e, review, _d, _s = reviews[path]
    assert game["result"] == "loss"
    kinds = {m["label"] for m in review["key_moments"]}
    assert kinds <= {"note", "missed_lethal"}
    notes = [m for m in review["key_moments"] if m["label"] == "note"]
    assert len(notes) <= 3, "design caps ledger moments at 3"


@pytest.mark.parametrize("path", ALL)
def test_turns_are_ordered_and_carry_a_ledger(reviews, path):
    _g, _e, review, _d, _s = reviews[path]
    turns = [t["turn"] for t in review["turns"]]
    assert turns == sorted(turns)
    assert len(turns) == len(set(turns)), "a turn was split in two"
    for t in review["turns"]:
        led = t["ledger"]
        assert led["mana_left"] >= 0
        assert led["unused_attacks"] >= 0
        assert t["decisions"], "a turn with no decisions was emitted"


@pytest.mark.parametrize("path", ALL)
def test_review_json_matches_the_ui_contract(reviews, path):
    _g, _e, review, _d, _s = reviews[path]
    for key in ("game_id", "status", "result", "matchup", "wp_series",
                "key_moments", "turns", "report", "evaluator_version",
                "labels_legend"):
        assert key in review, key
    assert review["status"] == "ready"
    assert set(review["matchup"]) >= {"us", "them"}
    # `i18n` mirrors the three English fields as (key, params) so the UI
    # can translate them; the English stays because stored reviews and
    # every current consumer read it.
    assert set(review["report"]) == {"headline", "bullets", "caveats",
                                     "i18n"}
    i18n = review["report"]["i18n"]
    assert i18n["headline"]["text"] == review["report"]["headline"]
    assert [m["text"] for m in i18n["bullets"]] == review["report"]["bullets"]
    assert [m["text"] for m in i18n["caveats"]] == review["report"]["caveats"]
    assert review["evaluator_version"] == cl.EVALUATOR_VERSION
    json.dumps(review)          # must survive the store round-trip


# ----------------------------------------------------------- the outcome
def test_read_outcome_finds_winner_and_lethal_turn():
    _raw, events, _s, _p = list(parsed_games(TAKEN))[0]
    out = read_outcome(events, us_pid=1)
    assert out["result"] == "win" and out["winner_pid"] == 1
    assert out["lethal_turn"] == out["end_turn"] == 7

    _raw, events, _s, _p = list(parsed_games(MISSED))[0]
    out = read_outcome(events, us_pid=1)
    assert out["result"] == "loss" and out["winner_pid"] == 2
    assert out["lethal_turn"] is None, \
        "we did not land the kill, so nothing is a played lethal"


def test_review_of_a_game_with_no_events_is_not_fatal(carddb):
    review, decisions, snaps = build_review(
        [], {"id": 1, "player_id": 1, "player_class": "MAGE",
             "opponent_class": "PRIEST", "result": "unknown"})
    assert review["turns"] == [] and decisions == [] and snaps == []
    assert review["report"]["caveats"]


# -------------------------------------------------------------- budget
@pytest.mark.parametrize("path", ALL)
def test_review_fits_the_five_second_budget(reviews, path):
    """Design §4.5: full post-game review <5 s typical. The fixture is
    already reviewed by the module fixture; re-time the assembly only."""
    import time
    game, events, _r, _d, _s = reviews[path]
    t0 = time.perf_counter()
    build_review(events, game)
    assert time.perf_counter() - t0 < 5.0


# ----------------------------------------------------------- snapshots
@pytest.mark.parametrize("path", ALL)
def test_snapshots_carry_wp_and_lethal_ok(reviews, path):
    """`/api/games/{id}/replay` reads these; without them the replay
    scrubber has nothing to scrub."""
    _g, _e, review, _d, snaps = reviews[path]
    assert len(snaps) == len(review["wp_series"])
    for row, pt in zip(snaps, review["wp_series"]):
        assert row["event_seq"] == pt["seq"]
        assert row["wp"] == pt["wp"]
        assert row["wp_source"] == "logistic_v1"
        assert row["search_ok"] == 0, "MVP invariant"
        assert isinstance(row["visible"], dict)
        assert isinstance(row["unimplemented"], list)


def test_snapshot_count_stays_inside_the_size_budget(reviews):
    """Design §2.5: decision points and turn boundaries only, not one
    per TAG_CHANGE."""
    _g, events, _r, _d, snaps = reviews[REAL2]
    assert len(snaps) < len(events) / 10


# ------------------------------------------------------------- timeout
def test_review_degrades_instead_of_hanging(carddb):
    """Design §4.6 bounds a review job. The budget switches off the deep
    lethal probe rather than killing the thread, so the ledger and the WP
    series still come out — and the report says what was skipped."""
    import json as _json
    import tempfile
    from capture.hslog_import import import_log as _import

    with tempfile.TemporaryDirectory() as tmp:
        st = Store(os.path.join(tmp, "t.sqlite"))
        try:
            gid = _import(st, REAL1)[0]
            game = dict(st.get_game(gid))
            events = [_json.loads(r["payload"])
                      for r in st.get_events(gid)]
        finally:
            st.close()

    review, decisions, snaps = build_review(events, game, timeout_s=-1)
    assert review["status"] == "partial"
    assert review["turns"] and decisions and snaps
    assert all(p["hatch"] for p in review["wp_series"])
    assert any("budget" in c for c in review["report"]["caveats"])
    # Nothing may be published from a run that did not finish searching.
    assert not [d for d in decisions if d["label"] == "missed_lethal"]


def test_a_finished_review_is_ready_not_partial(reviews):
    _g, _e, review, _d, _s = reviews[REAL1]
    assert review["status"] == "ready"
    assert not any("budget" in c for c in review["report"]["caveats"])


@pytest.mark.parametrize("path", ALL)
def test_every_key_moment_anchors_on_a_real_ply(reviews, path):
    """"Click a key moment" has to land somewhere; a null seq makes the
    interaction turn-granular for notes and ply-granular for lethals."""
    _g, _e, review, _d, _s = reviews[path]
    seqs = {p["seq"] for p in review["wp_series"]}
    turns = {t["turn"] for t in review["turns"]}
    for m in review["key_moments"]:
        assert m["turn"] in turns
        assert m["seq"] in seqs, f"{m['label']} points at {m['seq']}"


def test_the_review_is_written_from_our_side_whichever_player_we_are(
        carddb, tmp_path):
    """Every fixture happens to make us player 1. This pins the flip:
    the same log reviewed as player 2 must grade *their* turn as
    ungradeable and ours as the winning one."""
    import logging as _logging
    _logging.disable(_logging.WARNING)
    try:
        st = Store(str(tmp_path / "t.sqlite"))
        try:
            gid = import_log(st, MISSED,
                             player_name="Player2#00002")[0]
            game = dict(st.get_game(gid))
            events = [json.loads(r["payload"])
                      for r in st.get_events(gid)]
        finally:
            st.close()
    finally:
        _logging.disable(_logging.NOTSET)

    assert game["player_id"] == 2
    assert game["player_class"] == "ROGUE"
    assert game["result"] == "win" and game["going_first"] == 0

    review, decisions, _snaps = build_review(events, game)
    turn7 = [d for d in decisions if d["turn"] == 7]
    assert turn7 and all(d["side"] == "them" for d in turn7)
    assert all(d["label"] is None for d in turn7), \
        "the opponent's missed lethal is not ours to grade"
    ours = [d for d in decisions if d["side"] == "us"]
    assert ours and {d["label"] for d in ours} <= {None, "played_lethal"}
    assert "Thrown" not in review["report"]["headline"]


@pytest.mark.parametrize("path", REAL)
def test_lethal_ok_describes_the_state_not_whether_we_probed(reviews,
                                                             path):
    """`lethal_ok` means "a stats overlay of this position is sound"
    (design §2.5). Tying it to the two plies we happened to probe made
    every other ply show the UI's "lethal off" badge for no reason."""
    _g, _e, review, decisions, snaps = reviews[path]
    ok = [d for d in decisions if d["lethal_ok"]]
    assert len(ok) > len(decisions) * 0.5, (
        f"only {len(ok)}/{len(decisions)} plies sound — the flag is "
        f"tracking the probe again")
    checked = [dec for turn in review["turns"]
               for dec in turn["decisions"] if dec["lethal_checked"]]
    assert checked, "nothing was probed at all"
    assert len(checked) < len(decisions), (
        "every ply was probed; the deep search budget cannot afford that")
    # A snapshot and its decision must agree about the same position.
    by_seq = {d["event_seq"]: d["lethal_ok"] for d in decisions}
    for s in snaps:
        assert s["lethal_ok"] == by_seq[s["event_seq"]]


@pytest.mark.parametrize("path", ALL)
def test_a_review_is_deterministic(reviews, path):
    """Design §4.6: the engine is deterministic given a seed and the
    review copies rng state rather than reaching for the OS. Two runs
    over the same events must agree exactly, or "re-analyse" would keep
    changing the user's verdict."""
    game, events, first, dec_a, snap_a = reviews[path]
    second, dec_b, snap_b = build_review(events, game)
    drop = ("generated_at",)
    a = {k: v for k, v in first.items() if k not in drop}
    b = {k: v for k, v in second.items() if k not in drop}
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)
    assert json.dumps(dec_a, sort_keys=True) == \
        json.dumps(dec_b, sort_keys=True)
    assert json.dumps(snap_a, sort_keys=True) == \
        json.dumps(snap_b, sort_keys=True)
