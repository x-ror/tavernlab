"""PR 3 fixtures + PR 9 extensions for `hs2.lethal`.

The MVP's only skill label is *missed lethal*, so this solver is the one
piece of search the product actually publishes.  These pin the exact
positions rather than a win-rate, and they pin the honesty rules too:
a bounded search returns an `approx` plan, never a silent `None`.
"""
import pytest

from conftest import bare_game, give, place

from hs2.lethal import (Plan, best_burn, burn_options, execute,
                        find_lethal, hero_power_effect)


def opp_at(g, hp, armor=0):
    o = g.players[1]
    o.hp, o.armor = hp, armor
    return o


# ----------------------------------------------------------- burn to face
def test_burn_mage_face_lethal_is_found_and_exact():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 9)
    give(g, p, "Fireball")
    give(g, p, "Frostbolt")
    p.mana = 6
    plan = find_lethal(g, p)
    assert plan is not None
    assert not plan.approx
    names = sorted(a[1].card.name for a in plan if a[0] == "spell")
    assert names == ["Fireball", "Frostbolt"]
    assert all(a[2] is p.opponent for a in plan if a[0] == "spell")
    execute(g, p, plan)
    assert g.over and g.winner == p.idx


def test_burn_short_of_lethal_returns_none():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 10)
    give(g, p, "Fireball")
    give(g, p, "Frostbolt")
    p.mana = 6
    assert find_lethal(g, p) is None


def test_armor_counts_toward_the_requirement():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 6, armor=3)
    give(g, p, "Fireball")
    p.mana = 4
    assert find_lethal(g, p) is None
    opp_at(g, 6, armor=0)
    assert find_lethal(g, p) is not None


def test_knapsack_respects_mana():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 9)
    give(g, p, "Fireball")
    give(g, p, "Frostbolt")
    p.mana = 5           # cannot afford both (4 + 2)
    assert find_lethal(g, p) is None


def test_best_burn_is_exact_under_twelve_options():
    opts = [(6, 4, "fb"), (3, 2, "frost"), (1, 1, "ping")]
    dmg, picked = best_burn(opts, 6)
    assert dmg == 9 and set(picked) == {"fb", "frost"}
    dmg, picked = best_burn(opts, 3)
    assert dmg == 4 and set(picked) == {"frost", "ping"}
    assert best_burn([], 10) == (0, [])


def test_burn_options_skips_prepared_and_untargetable_spells():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    inst = give(g, p, "Fireball")[0]
    assert [o[2] for o in burn_options(p)] == [inst]
    inst.locked_turn = g.turn
    assert burn_options(p) == []


# ------------------------------------------------------------- taunt wall
def test_taunt_blocks_the_face_race():
    """Attacks cannot go face through a taunt; the design's abort case."""
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 6)
    place(g, p, "Wisp", atk=6, hp=6)
    place(g, o, "Imp Gang Stooge", atk=1, hp=8, taunt=True)
    assert find_lethal(g, p) is None


def test_taunt_is_cleared_then_face_when_the_maths_works():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 4)
    place(g, p, "Wisp", atk=4, hp=4)
    place(g, p, "Wisp", atk=4, hp=4)
    place(g, o, "Imp Gang Stooge", atk=1, hp=3, taunt=True)
    plan = find_lethal(g, p)
    assert plan is not None and not plan.approx
    hits = [a for a in plan if a[0] == "attack"]
    assert any(a[2] is o for a in hits), "nothing went face"
    assert any(a[2] is not o for a in hits), "taunt was not cleared"
    execute(g, p, plan)
    assert g.over and g.winner == p.idx


def test_spells_ignore_taunt():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 6)
    place(g, o, "Cyborg Patriarch", atk=3, hp=12, taunt=True)
    give(g, p, "Fireball")
    p.mana = 4
    plan = find_lethal(g, p)
    assert plan is not None
    assert [a[0] for a in plan] == ["spell"]


def test_divine_shield_taunt_needs_an_extra_hit():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 3)
    place(g, o, "Righteous Protector", atk=1, hp=1, taunt=True,
          divine_shield=True)
    place(g, p, "Wisp", atk=3, hp=3)
    assert find_lethal(g, p) is None, "shield packet was not counted"
    place(g, p, "Wisp", atk=3, hp=3)
    place(g, p, "Wisp", atk=3, hp=3)
    plan = find_lethal(g, p)
    assert plan is not None


def test_bounded_taunt_search_returns_approx_not_silent_none():
    """Design §3.5: 'don't return None silently — return approx=True'."""
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 5)
    for _ in range(4):                     # > 2 taunts trips the bound
        place(g, o, "Imp Gang Stooge", atk=1, hp=1, taunt=True)
    for _ in range(10):                    # > 9 swings trips it too
        place(g, p, "Wisp", atk=2, hp=2)
    plan = find_lethal(g, p)
    assert plan is not None, "bounded search went silent"
    assert plan.approx is True
    assert isinstance(plan, Plan) and isinstance(plan, list)


def test_immune_opponent_is_never_lethal():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 1)
    o.immune = True
    place(g, p, "Wisp", atk=10, hp=10)
    assert find_lethal(g, p) is None


# ------------------------------------------------------------ hero powers
def test_named_hero_power_face_damage_closes_the_gap():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 1)
    p.mana = 2
    plan = find_lethal(g, p)
    assert plan is not None
    hp = [a for a in plan if a[0] == "hero_power"]
    assert hp and hp[0][2].card.name == "Fireblast"


def test_unknown_hero_power_is_measured_on_a_clone_in_deep_mode():
    """Design §3.5: drive hero powers from `hero_power_use`, not a table
    of five names."""
    from hs2.lethal import _HP_PROBE
    g = bare_game("HUNTER", "PRIEST")
    p = g.players[0]
    hp = p.hero_power
    hp.card = type(hp.card)(
        id="probe_hp", name="Probe Blast", type="HERO_POWER",
        cls="HUNTER", cost=2, text="Deal 3 damage to the enemy hero.",
        implemented=True,
        hero_power_use=lambda gg, pp, t: gg.deal_damage(pp, pp.opponent, 3))
    hp.use_fn = hp.card.hero_power_use
    hp.cost = 2
    _HP_PROBE.pop("Probe Blast", None)

    assert hero_power_effect(g, p, hp, deep=False) == (0, 0, False)
    face, atk, cached = hero_power_effect(g, p, hp, deep=True)
    assert (face, atk, cached) == (3, 0, False)
    assert hero_power_effect(g, p, hp, deep=True) == (3, 0, True), \
        "second probe should hit the cache"

    opp_at(g, 3)
    p.mana = 2
    assert find_lethal(g, p, deep=False) is None
    plan = find_lethal(g, p, deep=True)
    assert plan is not None
    assert any(a[0] == "hero_power" for a in plan)


# -------------------------------------------------------- play-then-lethal
def test_charge_minion_in_hand_is_found_only_in_deep_mode():
    g = bare_game("DEMONHUNTER", "PRIEST")
    p = g.players[0]
    opp_at(g, 12)
    give(g, p, "TIME_020")            # Broxigar, 2 mana 12/12 Charge
    p.mana = 2
    assert find_lethal(g, p, deep=False) is None, \
        "the shallow path must not pay for clones"
    plan = find_lethal(g, p, deep=True)
    assert plan is not None
    assert plan.via_play == "Broxigar"
    assert plan[0][0] == "play"
    execute(g, p, plan)
    assert g.over and g.winner == p.idx


def test_deep_mode_does_not_invent_lethal_that_is_not_there():
    g = bare_game("DEMONHUNTER", "PRIEST")
    p = g.players[0]
    opp_at(g, 13)
    give(g, p, "TIME_020")
    p.mana = 2
    assert find_lethal(g, p, deep=True) is None


def test_deep_search_leaves_the_original_game_untouched():
    g = bare_game("DEMONHUNTER", "PRIEST")
    p = g.players[0]
    opp_at(g, 12)
    give(g, p, "TIME_020")
    p.mana = 2
    before = (len(p.hand), len(p.board), p.mana, g.players[1].hp)
    find_lethal(g, p, deep=True)
    assert (len(p.hand), len(p.board), p.mana,
            g.players[1].hp) == before


def test_plan_describe_is_human_readable():
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 6)
    give(g, p, "Fireball")
    p.mana = 4
    plan = find_lethal(g, p)
    assert "Fireball face" in plan.describe()


# ------------------------------------------- counterattacks while clearing
def test_an_attacker_that_dies_clearing_does_not_also_go_face():
    """Combat is simultaneous. Sending a 3/2 into a 5/5 taunt spends the
    minion; counting its 3 damage at the face too is how a false lethal
    gets published."""
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 3)
    place(g, p, "Wisp", atk=3, hp=2)
    place(g, o, "Imp Gang Stooge", atk=5, hp=3, taunt=True)
    assert find_lethal(g, p) is None


def test_a_survivor_still_goes_face():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 3)
    place(g, p, "Wisp", atk=3, hp=9)          # survives the trade
    place(g, p, "Wisp", atk=3, hp=3)
    place(g, o, "Imp Gang Stooge", atk=5, hp=3, taunt=True)
    plan = find_lethal(g, p)
    assert plan is not None
    execute(g, p, plan)
    assert g.over and g.winner == p.idx


def test_our_divine_shield_absorbs_the_counterattack():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 4)
    m = place(g, p, "Wisp", atk=4, hp=1)
    m.divine_shield = True
    m.windfury = True                          # clear, then face
    place(g, o, "Imp Gang Stooge", atk=9, hp=4, taunt=True,
          divine_shield=False)
    plan = find_lethal(g, p)
    assert plan is not None
    execute(g, p, plan)
    assert g.over and g.winner == p.idx


def test_a_zero_attack_poisonous_taunt_does_not_kill_through():
    g = bare_game("MAGE", "PRIEST")
    p, o = g.players[0], opp_at(g, 3)
    place(g, p, "Wisp", atk=3, hp=3)
    place(g, p, "Wisp", atk=3, hp=3)
    taunt = place(g, o, "Skywall Sentinel", atk=0, hp=3, taunt=True,
                  divine_shield=False)
    taunt.poisonous = True
    plan = find_lethal(g, p)
    assert plan is not None, "poison needs damage to trigger"
    execute(g, p, plan)
    assert g.over and g.winner == p.idx


def test_hero_power_target_is_resolved_when_the_plan_came_from_a_clone():
    """A plan built on a clone addresses clone entities. Executing it
    with an unresolved hero-power target damages the game we were only
    thinking about, and the real opponent never takes the hit."""
    g = bare_game("MAGE", "PRIEST")
    p = g.players[0]
    opp_at(g, 1)
    p.mana = 2
    plan = find_lethal(g, p)
    assert any(a[0] == "hero_power" for a in plan)
    clone = g.clone()
    before = (g.players[1].hp, g.players[1].armor, g.over)
    execute(clone, clone.players[0], plan)
    assert clone.over, "the clone should have died"
    assert (g.players[1].hp, g.players[1].armor, g.over) == before, \
        "executing on the clone reached back into the original"
