"""PR 8 / PR 9 / PR 10-L0: the overlay, the ledger, and the publish gates.

These pin the honesty rules as hard as the arithmetic, because the honesty
rules are the product: a wrong `lethal_ok` publishes a false accusation.
"""
import pytest

from eval import classify as cl
from eval import ledger
from eval.mapper import Overlay, build_overlay, _filler_deck
from eval.solvers import lethal as lethal_solver
from eval.types import EntityView, VisibleState


def minion(eid, pid, atk, hp, damage=0, **tags):
    t = {"CARDTYPE": "MINION"}
    t.update(tags)
    return EntityView(eid=eid, controller=pid, zone="PLAY", atk=atk,
                      health=hp, damage=damage, tags=t)


def card(eid, pid, card_id=None, name=None, cost=None):
    return EntityView(eid=eid, controller=pid, zone="HAND",
                      card_id=card_id, name=name, cost=cost,
                      tags={"CARDTYPE": "SPELL"})


def state(turn=7, us_hp=30, them_hp=10, crystals=6, used=0):
    vs = VisibleState(turn=turn, us=1, current_player=1)
    vs.heroes = {1: {"hp": us_hp, "armor": 0, "atk": 0, "attacks": 0},
                 2: {"hp": them_hp, "armor": 0, "atk": 0, "attacks": 0}}
    vs.mana = {1: {"crystals": crystals, "used": used},
               2: {"crystals": crystals, "used": 0}}
    vs.deck_counts = {1: 20, 2: 20}
    vs.boards = {1: [], 2: []}
    vs.hands = {1: [], 2: []}
    return vs


# ------------------------------------------------------------------ decks
def test_filler_deck_is_the_opponents_class_not_ours(carddb):
    d = _filler_deck("PRIEST")
    assert len(d.card_ids) == 30
    assert d.cls == "PRIEST"
    from hs2 import carddata
    classes = {carddata.DEFS[c].cls for c in d.card_ids}
    assert classes <= {"PRIEST", "NEUTRAL"}
    assert all(carddata.DEFS[c].implemented for c in d.card_ids)


def test_overlay_never_starts_the_game(carddb):
    """`start()` shuffles, mulligans and fires start-of-game effects on
    top of the board we are about to overwrite (design PR 8)."""
    vs = state()
    vs.boards[1] = [minion(10, 1, 3, 4)]
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    assert len(ov.us.hand) == 0, "a mulligan ran"
    assert len(ov.them.hand) == 0
    assert "The Coin" not in [i.card.name for i in ov.them.hand]
    assert ov.us.marks.get("start_hand") is None, "start() was called"
    assert [m.attack for m in ov.us.active_minions] == [3]
    assert ov.game.turn == 7


def test_overlay_uses_the_log_turn_not_a_hardcoded_ten(carddb):
    for turn in (1, 5, 23):
        ov = build_overlay(state(turn=turn), us_cls="MAGE",
                           them_cls="PRIEST")
        assert ov.game.turn == turn


def test_search_ok_is_never_set_by_a_stats_overlay(carddb):
    ov = build_overlay(state(), us_cls="MAGE", them_cls="PRIEST")
    assert ov.search_ok is False
    assert Overlay.__slots__.count("search_ok") == 1


def test_board_keywords_come_from_log_tags_not_the_carddef(carddb):
    """An unimplemented card still has a truthful ATK/HP/TAUNT in the
    log; that is all `find_lethal` reads."""
    vs = state()
    vs.boards[2] = [minion(20, 2, 4, 7, card_id_missing=True, TAUNT=1,
                           DIVINE_SHIELD=1, WINDFURY=1)]
    vs.boards[2][0].card_id = "NOT_A_REAL_CARD"
    vs.boards[2][0].name = "Unknown Thing"
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    m = ov.them.active_minions[0]
    assert (m.attack, m.health) == (4, 7)
    assert m.taunt and m.divine_shield and m.windfury
    assert m.deathrattles == [] and m.triggers == {}, \
        "an overlay must not fire behaviour it never registered"


def test_mana_accounts_for_used_temp_and_overload(carddb):
    vs = state(crystals=8, used=3)
    vs.mana[1]["temp"] = 2
    vs.mana[1]["overload"] = 1
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    assert ov.us.crystals == 8
    assert ov.us.mana == 8 - 3 - 1 + 2


def test_missing_hero_fields_turn_lethal_ok_off(carddb):
    vs = state()
    vs.heroes[2] = {}
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    assert ov.lethal_ok is False
    assert any("did not parse" in r for r in ov.reasons)


def test_unimplemented_hand_card_clears_hand_complete_only(carddb):
    vs = state()
    vs.hands[1] = [card(64, 1, card_id="CORE_CS2_029", name="Fireball",
                        cost=4),
                   card(65, 1, card_id="ZZZ_NOT_REAL", name="Mystery",
                        cost=3)]
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    assert ov.lethal_ok is True, \
        "an unmodelled card can only add damage, never invent it"
    assert ov.hand_complete is False
    assert "Mystery" in ov.unimplemented
    assert [i.card.name for i in ov.us.hand] == ["Fireball"]


def test_hand_cost_override_is_carried_as_cost_delta(carddb):
    vs = state()
    vs.hands[1] = [card(64, 1, card_id="CORE_CS2_029", name="Fireball",
                        cost=1)]
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    inst = ov.us.hand[0]
    assert ov.us.effective_cost(inst) == 1


def test_opponent_hand_is_never_invented(carddb):
    vs = state()
    vs.hands[2] = [EntityView(eid=90 + i, controller=2, zone="HAND")
                   for i in range(6)]
    ov = build_overlay(vs, us_cls="MAGE", them_cls="PRIEST")
    assert ov.them.hand == []


# ---------------------------------------------------------- lethal solver
def test_missed_lethal_is_detected_and_publishable(carddb):
    vs = state(them_hp=6)
    vs.hands[1] = [card(64, 1, card_id="CORE_CS2_029", name="Fireball",
                        cost=4)]
    f = lethal_solver.detect(vs, us_cls="MAGE", them_cls="PRIEST")
    assert f.available and f.lethal_ok and f.missed
    assert "Fireball" in f.plan
    v = cl.classify(cl.gates_for(lethal=f.to_dict()))
    assert v.label == "missed_lethal" and v.conf >= 0.9


def test_taken_lethal_is_a_quiet_positive_not_an_accusation(carddb):
    vs = state(them_hp=6)
    vs.hands[1] = [card(64, 1, card_id="CORE_CS2_029", name="Fireball",
                        cost=4)]
    f = lethal_solver.detect(vs, us_cls="MAGE", them_cls="PRIEST",
                             taken=True)
    assert f.available and not f.missed
    assert cl.classify(cl.gates_for(lethal=f.to_dict())).label == \
        "played_lethal"


def test_no_lethal_publishes_nothing(carddb):
    vs = state(them_hp=30)
    f = lethal_solver.detect(vs, us_cls="MAGE", them_cls="PRIEST")
    assert not f.available and not f.missed
    v = cl.classify(cl.gates_for(lethal=f.to_dict()))
    assert v.label is None
    assert "search_ok=0" in v.reason


def test_lethal_ok_false_suppresses_the_label(carddb):
    vs = state(them_hp=6)
    vs.heroes[1] = {}                      # hero did not parse
    vs.hands[1] = [card(64, 1, card_id="CORE_CS2_029", name="Fireball",
                        cost=4)]
    f = lethal_solver.detect(vs, us_cls="MAGE", them_cls="PRIEST")
    assert not f.lethal_ok and not f.missed
    assert cl.classify(cl.gates_for(lethal=f.to_dict())).label is None


def test_solver_never_raises_on_a_junk_state(carddb):
    junk = VisibleState(turn=3, us=1)
    f = lethal_solver.detect(junk, us_cls="MAGE", them_cls="PRIEST")
    assert f.missed is False


def test_opponent_already_dead_is_not_a_missed_lethal(carddb):
    vs = state(them_hp=0)
    f = lethal_solver.detect(vs, us_cls="MAGE", them_cls="PRIEST")
    assert not f.available
    assert "already at 0" in " ".join(f.reasons)


# ------------------------------------------------------------------ ledger
def test_ledger_counts_mana_attacks_and_affordable_cards():
    vs = state(crystals=6, used=4)
    vs.boards[1] = [minion(10, 1, 3, 3),
                    minion(11, 1, 2, 2, NUM_ATTACKS_THIS_TURN=1)]
    vs.hands[1] = [card(20, 1, name="Frostbolt", cost=2),
                   card(21, 1, name="Big Thing", cost=8)]
    led = ledger.build(vs, hero_power_used=True)
    assert led.mana_left == 2
    assert led.unused_attacks == 1
    assert led.unused_attackers == ["#10"], led.unused_attackers
    assert [c["card"] for c in led.affordable_unplayed] == ["Frostbolt"]
    assert any("mana_waste" in n.tags for n in led.notes)


def test_ledger_ignores_summoning_sick_and_frozen_minions():
    vs = state()
    vs.boards[1] = [minion(10, 1, 3, 3, EXHAUSTED=1),
                    minion(11, 1, 3, 3, FROZEN=1),
                    minion(12, 1, 0, 3),
                    minion(13, 1, 3, 3, CANT_ATTACK=1)]
    led = ledger.build(vs)
    assert led.unused_attacks == 0


def test_charge_minion_is_not_treated_as_summoning_sick():
    vs = state()
    vs.boards[1] = [minion(10, 1, 5, 5, EXHAUSTED=1, CHARGE=1)]
    assert ledger.build(vs).unused_attacks == 1


def test_windfury_counts_both_swings():
    vs = state()
    vs.boards[1] = [minion(10, 1, 3, 3, WINDFURY=1)]
    assert ledger.build(vs).unused_attacks == 2
    vs.boards[1][0].tags["NUM_ATTACKS_THIS_TURN"] = 1
    assert ledger.build(vs).unused_attacks == 1


def test_face_down_cards_are_never_counted_as_affordable():
    vs = state(crystals=6, used=0)
    vs.hands[1] = [EntityView(eid=30, controller=1, zone="HAND")]
    led = ledger.build(vs)
    assert led.affordable_unplayed == []


def test_notes_carry_a_caveat_not_a_verdict():
    vs = state(crystals=6, used=4)
    vs.hands[1] = [card(20, 1, name="Frostbolt", cost=2)]
    note = ledger.build(vs).notes[0]
    assert "note" in note.tags
    assert note.caveats and "not a verdict" in note.caveats[0]
    assert not any(t in note.tags for t in ("blunder", "mistake"))


# -------------------------------------------------------------- classifier
def test_only_three_labels_ship_in_mvp():
    assert set(cl.MVP_LABELS) == {"missed_lethal", "played_lethal", "note"}
    for spec in cl.LABELS:
        if spec.phase == "mvp":
            continue
        assert spec.key in cl.COMING_SOON


def test_delta_wp_alone_can_never_produce_a_glyph():
    g = cl.gates_for(delta_wp=-0.9)
    assert cl.classify(g).label is None
    g = cl.gates_for(delta_wp=-0.9, actions_complete=True, search_depth=3)
    assert cl.classify(g).label is None, \
        "search_ok is the gate; MVP never sets it"


def test_wp_caveat_states_the_model_and_that_it_is_hatched():
    c = cl.wp_caveat()
    assert "9 weights" in c and "hatched" in c and "66.7%" in c


def test_legend_marks_future_labels_unavailable():
    entries = {e["key"]: e for e in cl.legend()}
    assert entries["blunder"]["available"] is False
    assert "search_ok" in entries["blunder"]["needs"]
    assert entries["missed_lethal"]["available"] is True
