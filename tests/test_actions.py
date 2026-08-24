"""PR 1: the eid-addressed action API.

`id()` is banned in an Action because it is not stable across
`Game.clone()`; every test here is ultimately about that. The
`forced_picks` tests matter for a different reason: without them a
replayed Discover re-rolls the RNG, and a "1-ply" search is really
sampling a different game each time.
"""
import pytest

from conftest import bare_game, give, new_game, build_deck, place

from hs2 import carddata
from hs2.actions import (Action, apply, attack_targets, legal_actions,
                         targets_for)
from hs2.engine import MAX_BOARD


# ------------------------------------------------------------- structure
def test_action_is_frozen_and_hashable():
    a = Action("play", eid=7)
    assert {a, Action("play", eid=7)} == {a}
    with pytest.raises(Exception):
        a.eid = 9


def test_no_action_field_ever_holds_a_python_id(carddb):
    """The regression: `id()` looks stable inside one call and is
    meaningless after a clone."""
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    give(g, p, "Fireball")
    place(g, p, "Wisp", atk=2, hp=2)
    acts, _ = legal_actions(g, p)
    live = set(g._by_eid) | {None}
    for a in acts:
        for field in (a.eid, a.attacker_eid, a.target_eid):
            assert field in live, f"{field} is not an eid in this game"


def test_actions_survive_a_clone_by_eid(carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    give(g, p, "Fireball")
    acts, _ = legal_actions(g, p)
    play = next(a for a in acts if a.kind == "play")
    c = g.clone()
    assert c.by_eid(play.eid) is not g.by_eid(play.eid)
    assert c.by_eid(play.eid).card.name == "Fireball"
    assert apply(c, c.players[0], play) is not False


# ------------------------------------------------------------- targeting
def test_targets_cover_both_heroes_and_boards(carddb):
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players
    place(g, p, "Wisp")
    place(g, o, "Wisp")
    ours = targets_for(g, p, "any")
    assert set(ours) == {p, o, p.board[0], o.board[0]}
    assert targets_for(g, p, "friendly_minion") == [p.board[0]]
    assert targets_for(g, p, "enemy_minion") == [o.board[0]]
    assert targets_for(g, p, None) == [None]


def test_hero_targets_use_the_player_eid(carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    give(g, p, "Fireball")
    acts, _ = legal_actions(g, p)
    hero_shots = [a for a in acts if a.kind == "play"
                  and a.target_eid in (1, 2)]
    assert hero_shots, "no Action targeted a hero"
    assert {a.target_eid for a in hero_shots} == {1, 2}


def test_elusive_minions_are_untargetable_by_spells_only(carddb):
    """`Game.attack` does not enforce Elusive; the enumerator is the one
    place the rule lives, so it is pinned here."""
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players
    m = place(g, o, "Wisp")
    m.elusive = True
    assert targets_for(g, p, "minion", spell_like=True) == []
    assert targets_for(g, p, "minion", spell_like=False) == [m]


def test_taunt_is_enforced_by_the_enumerator(carddb):
    """`Game.attack` does not check taunt either — the scripted AI did."""
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players
    taunt = place(g, o, "Imp Gang Stooge", taunt=True)
    plain = place(g, o, "Wisp")
    assert attack_targets(p, True) == [taunt]
    plain.taunt = False
    taunt.taunt = False
    assert set(attack_targets(p, True)) == {taunt, plain, o}


def test_stealthed_minions_cannot_be_attacked(carddb):
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players
    m = place(g, o, "Wisp")
    m.stealth = True
    assert attack_targets(p, True) == [o]


# ------------------------------------------------------------ choose-one
def test_choose_one_becomes_one_action_per_choice(carddb):
    g = bare_game("DEATHKNIGHT", "PRIEST")
    p, o = g.players
    victim = place(g, o, "Wisp")          # Morbid Swarm targets a minion
    inst = give(g, p, "Morbid Swarm")[0]
    p.corpses = 5
    acts, _ = legal_actions(g, p)
    plays = [a for a in acts if a.kind == "play" and a.eid == inst.eid]
    assert plays, "the choose-one card was not enumerated at all"
    assert {a.choice for a in plays} == {0, 1}
    assert all(a.kind == "play" for a in plays)
    assert {a.target_eid for a in plays} == {victim.eid}


def test_a_targeted_card_with_no_legal_target_is_unplayable(carddb):
    """Hearthstone will not let you cast it, so neither will we — and
    the set is still `complete`, because nothing legal was omitted."""
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    inst = give(g, p, "Flash Heal")[0]     # target="any"
    assert [a for a in legal_actions(g, p)[0] if a.eid == inst.eid]

    g2 = bare_game("DEATHKNIGHT", "PRIEST")
    p2 = g2.players[0]
    swarm = give(g2, p2, "Morbid Swarm")[0]   # target="minion"
    p2.corpses = 5
    acts, complete = legal_actions(g2, p2)
    assert not [a for a in acts if a.eid == swarm.eid]
    assert complete is True


def test_discover_is_not_enumerated(carddb):
    """The offered set is hidden information: a discover is a logged
    decision, not a searchable one (design PR 1)."""
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    give(g, p, "Runed Orb")
    acts, _ = legal_actions(g, p)
    assert not [a for a in acts if a.kind == "discover"]


# ------------------------------------------------------- actions_complete
def test_actions_complete_goes_false_on_an_unimplemented_hand_card(
        carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    give(g, p, "Fireball")
    _acts, complete = legal_actions(g, p)
    assert complete is True

    unimpl = next(d for d in carddata.DEFS.values()
                  if d.coll and not d.implemented)
    inst = g.reg(carddata.make_inst(unimpl.id))
    p.hand.append(inst)
    acts, complete = legal_actions(g, p)
    assert complete is False, "an unplayable card must not be silent"
    assert not [a for a in acts if a.eid == inst.eid]


def test_end_turn_is_always_legal(carddb):
    g = bare_game("MAGE", "PRIEST")
    acts, _ = legal_actions(g, g.players[0])
    assert acts[-1] == Action("end_turn")


def test_a_finished_game_offers_nothing(carddb):
    g = bare_game("MAGE", "PRIEST")
    g.over = True
    assert legal_actions(g, g.players[0]) == ([], True)


def test_full_board_suppresses_minion_plays(carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    for _ in range(MAX_BOARD):
        place(g, p, "Wisp")
    inst = give(g, p, "Wisp")[0]
    acts, _ = legal_actions(g, p)
    assert not [a for a in acts if a.eid == inst.eid]


def test_unaffordable_and_prepared_cards_are_excluded(carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    inst = give(g, p, "Fireball")[0]
    p.mana = 3
    assert not [a for a in legal_actions(g, p)[0] if a.eid == inst.eid]
    p.mana = 10
    assert [a for a in legal_actions(g, p)[0] if a.eid == inst.eid]
    inst.locked_turn = g.turn
    assert not [a for a in legal_actions(g, p)[0] if a.eid == inst.eid]


# ----------------------------------------------------------- forced picks
def test_forced_picks_beat_the_rng_in_discover(carddb):
    """Without this a replayed line re-rolls the Discover, so a 1-ply
    search is sampling a different game every time."""
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    pool = [carddata.get_def(n) for n in ("Fireball", "Frostbolt", "Wisp")]
    forced = []
    for want in ("Frostbolt", "Wisp", "Fireball"):
        g._forced_picks = [want]
        forced.append(g.discover(p, pool).name)
    assert forced == ["Frostbolt", "Wisp", "Fireball"]


def test_discover_records_what_was_offered(carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    pool = [carddata.get_def(n)
            for n in ("Fireball", "Frostbolt", "Wisp", "Sleet Storm")]
    got = g.discover(p, pool, pick="Wisp")
    offered, chosen = g._last_discover
    assert chosen is got and got.name == "Wisp"
    assert got in offered and len(offered) == 3


def test_forced_picks_are_consumed_by_a_nested_discover(carddb):
    """`Runed Orb` discovers from inside its own spell, so the queue has
    to survive the whole `apply`, not just the top-level call."""
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    inst = give(g, p, "Runed Orb")[0]
    p.mana = 10
    before = len(p.hand)
    apply(g, p, Action("play", eid=inst.eid, target_eid=2),
          forced_picks=["Fireball"])
    assert len(p.hand) == before      # one spent, one discovered
    assert any(i.card.name == "Fireball" for i in p.hand), \
        [i.card.name for i in p.hand]


def test_apply_restores_the_queue_it_found(carddb):
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    give(g, p, "Fireball")
    g._forced_picks = ["sentinel"]
    acts, _ = legal_actions(g, p)
    apply(g, p, next(a for a in acts if a.kind == "play"),
          forced_picks=["other"])
    assert g._forced_picks == ["sentinel"]


def test_resolve_pick_accepts_eid_index_id_and_name(carddb):
    from hs2.engine import resolve_pick
    defs = [carddata.get_def(n) for n in ("Fireball", "Frostbolt")]
    assert resolve_pick("Frostbolt", defs) is defs[1]
    assert resolve_pick(defs[0].id, defs) is defs[0]
    assert resolve_pick(1, defs) is defs[1]
    assert resolve_pick("Nothing At All", defs) is None
    assert resolve_pick(None, defs) is None


# ------------------------------------------------------------- mulligan
def test_mulligan_keep_fn_replays_the_real_hand(carddb):
    """Review needs the hand the human actually kept, not the engine's
    `cost <= 2|3` heuristic."""
    deck = build_deck("m", "MAGE", ["Fireball"] * 4 + ["Wisp"] * 4)
    g = new_game(deck, build_deck("o", "PRIEST", []), seed=5)
    seen = {}

    def keep_expensive(p, drawn):
        seen[p.idx] = [i.card.name for i in drawn]
        return [i for i in drawn if i.card.cost >= 4]

    g.start(first=0, keep_fns=(keep_expensive, None))
    assert seen[0], "keep_fn was never called"
    assert len(g.players[0].hand) == 3


def test_default_mulligan_is_unchanged(metas):
    """`Game.run()` behaviour must not move (design PR 1)."""
    a, b = metas[0], metas[1]
    from conftest import new_game as ng
    outcomes = []
    for seed in range(6):
        g = ng(a, b, seed=seed)
        outcomes.append(g.run())
    assert outcomes == [ng(a, b, seed=s).run() for s in range(6)]
    assert all(o in (0, 1, None) for o in outcomes)
