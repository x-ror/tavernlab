"""PR 3: the VisibleState -> winprob conversion.

The bug this pins: `winprob_raw` takes *your* turn number and doubles it
internally, while `features()` takes the engine's `Game.turn`, which the
HS `GameEntity` TURN tag already matches.  Feeding a VisibleState turn
into `winprob_raw` silently shifts the model by up to ten half-turns.
"""
import pytest

from conftest import build_deck, new_game

from eval.types import EntityView, VisibleState
from hs2.winprob import (FEATS, features, features_from_visible,
                         winprob_raw, wp_from_visible)


def minion(eid, pid, atk, hp, damage=0, taunt=False):
    tags = {"CARDTYPE": "MINION"}
    if taunt:
        tags["TAUNT"] = 1
    return EntityView(eid=eid, controller=pid, zone="PLAY", atk=atk,
                      health=hp, damage=damage, tags=tags)


def sample_state(turn=6):
    vs = VisibleState(turn=turn, us=1, current_player=1)
    vs.heroes = {1: {"hp": 24, "armor": 3}, 2: {"hp": 18, "armor": 0}}
    vs.boards = {1: [minion(10, 1, 3, 4), minion(11, 1, 2, 2, taunt=True)],
                 2: [minion(20, 2, 5, 5, damage=2)]}
    vs.hands = {1: [EntityView(eid=30 + i, controller=1, zone="HAND")
                    for i in range(4)],
                2: [EntityView(eid=40 + i, controller=2, zone="HAND")
                    for i in range(6)]}
    vs.deck_counts = {1: 17, 2: 12}
    vs.weapons = {1: {"atk": 3, "dur": 2}}
    return vs


def test_feature_names_are_the_documented_nine():
    assert FEATS == ["bias", "hp_diff", "board_atk_diff", "board_hp_diff",
                     "hand_diff", "deck_diff", "turn", "has_weapon",
                     "taunt_hp_diff"]
    assert len(features_from_visible(sample_state())) == len(FEATS) == 9


def test_visible_features_match_a_live_game_at_the_same_turn(metas):
    """`vs.turn == 6` must line up with `Game.turn == 6`, feature by
    feature — not with a game at turn 12."""
    from hs2.engine import Minion

    g = new_game(build_deck("a", "MAGE", []), build_deck("b", "ROGUE", []),
                 seed=2)
    g.start(first=0)
    g.turn = 6
    us, them = g.players[0], g.players[1]
    us.hp, us.armor = 24, 3
    them.hp, them.armor = 18, 0
    for p in (us, them):
        p.board.clear()
        p.hand.clear()
        p.deck.clear()

    def put(p, atk, hp, damage=0, taunt=False):
        from hs2 import carddata
        m = Minion(carddata.get_def("Wisp"), p)
        m.atk_base, m.hp_base, m.damage = atk, hp, damage
        m.taunt = taunt
        m.just_summoned = False
        p.board.append(g.reg(m))

    put(us, 3, 4)
    put(us, 2, 2, taunt=True)
    put(them, 5, 5, damage=2)
    from hs2.carddata import make_inst_by_name
    us.hand.extend(make_inst_by_name("Wisp") for _ in range(4))
    them.hand.extend(make_inst_by_name("Wisp") for _ in range(6))
    us.deck.extend(make_inst_by_name("Wisp") for _ in range(17))
    them.deck.extend(make_inst_by_name("Wisp") for _ in range(12))

    class W:
        atk, windfury, lifesteal = 3, False, False
    us.weapon = W()

    live = features(g, us)
    vis = features_from_visible(sample_state(turn=6))
    for name, a, b in zip(FEATS, live, vis):
        assert a == pytest.approx(b), f"{name}: live {a} vs visible {b}"


def test_visible_turn_is_not_winprob_raw_my_turn():
    """The regression the design calls out: `winprob_raw` doubles."""
    vs = sample_state(turn=6)
    raw_args = {"hp_diff": 27 - 18, "board_atk_diff": 5 - 5,
                "board_hp_diff": (4 + 2) - 3, "hand_diff": 4 - 6,
                "deck_diff": 17 - 12, "my_turn": 6, "has_weapon": True,
                "taunt_hp_diff": 2 - 0}
    assert wp_from_visible(vs) != pytest.approx(winprob_raw(raw_args))
    # …and it *does* match once the doubling is undone.
    assert wp_from_visible(vs) == pytest.approx(
        winprob_raw(dict(raw_args, my_turn=3)))


def test_turn_term_saturates_at_twenty():
    a = features_from_visible(sample_state(turn=20))[FEATS.index("turn")]
    b = features_from_visible(sample_state(turn=45))[FEATS.index("turn")]
    assert a == b == 1.0


def test_wp_is_a_probability_and_moves_the_right_way():
    ahead = sample_state()
    behind = sample_state()
    behind.heroes = {1: {"hp": 4, "armor": 0}, 2: {"hp": 30, "armor": 5}}
    for vs in (ahead, behind):
        assert 0.0 < wp_from_visible(vs) < 1.0
    assert wp_from_visible(ahead) > wp_from_visible(behind)


def test_us_pid_flips_the_perspective():
    vs = sample_state()
    assert wp_from_visible(vs, 1) == pytest.approx(
        1.0 - wp_from_visible(vs, 2), abs=0.35)
    assert wp_from_visible(vs, 1) != wp_from_visible(vs, 2)


def test_dormant_minions_do_not_count():
    vs = sample_state()
    d = minion(99, 1, 8, 8)
    d.tags["DORMANT"] = 1
    vs.boards[1].append(d)
    assert features_from_visible(vs) == features_from_visible(sample_state())
