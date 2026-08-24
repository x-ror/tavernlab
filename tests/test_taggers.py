"""Design §3.6: strategic taggers, and the bar they have to clear.

"Wrong strategic tags are worse than none" — so most of these tests are
about the tagger staying **quiet**: on a slow board, on an even race, on
a coin turn.  The evidence dict is asserted alongside the text because a
Legend player has to be able to check the claim.
"""
import pytest

from eval.taggers import (HEAVY_KEEP_COST, MAX_MEANINGFUL_CLOCK,
                          MIN_CLOCK_GAP, all_tags, beatdown,
                          mulligan_keep, removal_vs_face)
from eval.types import EntityView, VisibleState


def minion(eid, pid, atk, hp, **tags):
    t = {"CARDTYPE": "MINION"}
    t.update(tags)
    return EntityView(eid=eid, controller=pid, zone="PLAY", atk=atk,
                      health=hp, tags=t)


def board(us_hp=30, them_hp=30, ours=(), theirs=(), us_atk=0):
    vs = VisibleState(turn=8, us=1, current_player=1)
    vs.heroes = {1: {"hp": us_hp, "armor": 0, "atk": us_atk},
                 2: {"hp": them_hp, "armor": 0, "atk": 0}}
    vs.boards = {1: [minion(10 + i, 1, a, h) for i, (a, h)
                     in enumerate(ours)],
                 2: [minion(20 + i, 2, a, h) for i, (a, h)
                     in enumerate(theirs)]}
    vs.hands = {1: [], 2: []}
    return vs


# ------------------------------------------------------------- beatdown
def test_we_are_the_beatdown_when_our_clock_is_clearly_faster():
    vs = board(us_hp=28, them_hp=10, ours=[(6, 6), (4, 4)],
               theirs=[(2, 2)])
    tag = beatdown(vs)
    assert tag["tag"] == "beatdown_us" and tag["polarity"] == 1
    ev = tag["evidence"]
    assert ev["our_face_damage"] == 10 and ev["our_clock"] == 1
    assert ev["their_face_damage"] == 2 and ev["their_clock"] == 14
    assert "beatdown" in tag["text"]


def test_they_are_the_beatdown_when_theirs_is():
    vs = board(us_hp=10, them_hp=30, ours=[(1, 1)], theirs=[(5, 5)])
    tag = beatdown(vs)
    assert tag["tag"] == "beatdown_them" and tag["polarity"] == -1
    assert tag["evidence"]["their_clock"] == 2


def test_a_slow_board_says_nothing():
    """A lone 1/1 chipping for 30 turns is not a beatdown call."""
    vs = board(ours=[(1, 1)], theirs=[])
    tag = beatdown(vs)
    assert tag["polarity"] == 0
    assert tag["evidence"]["our_clock"] > MAX_MEANINGFUL_CLOCK
    assert "No real clock yet" in tag["text"]
    assert all_tags(vs) == []


def test_an_even_race_says_nothing():
    vs = board(us_hp=10, them_hp=10, ours=[(5, 5)], theirs=[(5, 5)])
    tag = beatdown(vs)
    assert tag["tag"] == "beatdown_race" and tag["polarity"] == 0
    assert abs(tag["evidence"]["our_clock"]
               - tag["evidence"]["their_clock"]) < MIN_CLOCK_GAP


def test_empty_boards_say_nothing():
    tag = beatdown(board())
    assert tag["polarity"] == 0 and "Neither side" in tag["text"]


def test_armour_and_hero_attack_count():
    vs = board(them_hp=8, ours=[(4, 4)], us_atk=4)
    assert beatdown(vs)["evidence"]["our_face_damage"] == 8
    vs.heroes[2]["armor"] = 20
    assert beatdown(vs)["evidence"]["their_hp"] == 28


@pytest.mark.parametrize("blocker", ["FROZEN", "CANT_ATTACK", "DORMANT"])
def test_minions_that_cannot_swing_are_not_a_clock(blocker):
    vs = board(them_hp=10, ours=[(6, 6)])
    vs.boards[1][0].tags[blocker] = 1
    assert beatdown(vs)["evidence"]["our_face_damage"] == 0


def test_windfury_counts_twice():
    vs = board(them_hp=10, ours=[(4, 4)])
    vs.boards[1][0].tags["WINDFURY"] = 1
    assert beatdown(vs)["evidence"]["our_face_damage"] == 8


# ------------------------------------------------------- removal vs face
def test_removal_vs_face_is_silent_when_the_beatdown_is_unclear():
    assert removal_vs_face(board()) is None
    assert removal_vs_face(board(ours=[(1, 1)])) is None


def test_removal_vs_face_agrees_or_disagrees_with_the_actual_play():
    vs = board(us_hp=28, them_hp=10, ours=[(6, 6), (4, 4)],
               theirs=[(2, 2)])
    agree = removal_vs_face(vs, target_is_face=True)
    assert "matches this play" in agree["text"]
    disagree = removal_vs_face(vs, target_is_face=False)
    assert "went the other way" in disagree["text"]
    assert agree["evidence"]["our_face_damage"] == 10


# -------------------------------------------------------------- mulligan
def test_heavy_keep_on_the_play_is_flagged():
    choices = [{"picked": True, "cost": 7, "name": "Big Thing"},
               {"picked": True, "cost": 1, "name": "Cheap"},
               {"picked": False, "cost": 8, "name": "Tossed"}]
    tag = mulligan_keep(choices, going_first=True, archetype="Herald Rogue")
    assert tag["tag"] == "mull_keep_heavy" and tag["polarity"] == -1
    assert tag["evidence"]["cards"] == ["Big Thing"]
    assert "Herald Rogue" in tag["text"]
    assert "Tossed" not in tag["text"]


def test_the_coin_turns_the_checklist_off():
    """With the coin the curve is a turn cheaper; the checklist is
    specifically 'on the play'."""
    choices = [{"picked": True, "cost": 7, "name": "Big Thing"}]
    assert mulligan_keep(choices, going_first=False) is None
    assert mulligan_keep(choices, going_first=True) is not None


def test_cheap_keeps_say_nothing():
    choices = [{"picked": True, "cost": HEAVY_KEEP_COST - 1,
                "name": "Fine"}]
    assert mulligan_keep(choices, going_first=True) is None
    assert mulligan_keep([], going_first=True) is None
    assert mulligan_keep(None, going_first=True) is None


def test_unnamed_cards_are_skipped_not_assumed_cheap():
    """The opponent's mulligan is face-down; a missing cost must not be
    read as 'cost 0'."""
    choices = [{"picked": True, "cost": None, "card_id": None}]
    assert mulligan_keep(choices, going_first=True) is None


# ---------------------------------------------------------------- wiring
def test_review_only_publishes_checkable_strategic_tags(carddb):
    """End-to-end: the tags that reach the review must all carry a clock
    inside the meaningful window."""
    import json
    import logging
    import tempfile
    import os
    from capture.hslog_import import import_log
    from eval.review import build_review
    from store import Store

    logging.disable(logging.WARNING)
    fix = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "logs", "fixtures", "real_game1.log.gz")
    try:
        with tempfile.TemporaryDirectory() as tmp:
            st = Store(os.path.join(tmp, "t.sqlite"))
            try:
                gid = import_log(st, fix)[0]
                game = dict(st.get_game(gid))
                events = [json.loads(r["payload"])
                          for r in st.get_events(gid)]
            finally:
                st.close()
        _review, decisions, _snaps = build_review(events, game)
    finally:
        logging.disable(logging.NOTSET)

    tagged = [d for d in decisions if d["explanation"]["strategic"]]
    assert tagged, "no strategic tag survived the gates"
    for d in tagged:
        for line in d["explanation"]["strategic"]:
            assert "No real clock yet" not in line
            assert "Neither side" not in line
        assert set(d["explanation"]["tags"]) & {
            "beatdown_us", "beatdown_them", "removal_vs_face",
            "mull_keep_heavy"}
