"""Missed-lethal precision (design §3.8: ≥95%, "a false lethal call is a
product-ending bug").

Unit fixtures pin the positions we thought of. This pins the ones we did
not: random boards, and for every plan the solver returns, **execute it on
a clone and check the opponent actually died**. A plan that does not kill
is a false positive, and the bar for those is zero, not 5%.

The 95% figure in the design is the bar against *human* games, which we
cannot measure without a labelled corpus — see the module note at the end.
"""
import random

import pytest

from conftest import bare_game, give, place

from hs2 import carddata
from hs2.lethal import Plan, execute, find_lethal

MINIONS = ("Wisp", "Imp Gang Stooge", "Annoy-o-Tron",
           "Righteous Protector", "Skywall Sentinel")
BURN = ("Fireball", "Frostbolt", "Arcane Barrage")
ROUNDS = 1200


def random_position(rng):
    g = bare_game("MAGE", "PRIEST", seed=rng.randrange(10 ** 6))
    p, o = g.players
    o.hp = rng.randint(1, 22)
    o.armor = rng.choice([0, 0, 0, 2, 5])
    p.mana = rng.randint(0, 10)
    p.hero_attacks = 0
    for _ in range(rng.randint(0, 5)):
        m = place(g, p, rng.choice(MINIONS),
                  atk=rng.randint(0, 8), hp=rng.randint(1, 8))
        m.windfury = rng.random() < 0.15
        if rng.random() < 0.2:
            m.just_summoned = True
            m.charge = rng.random() < 0.5
    for _ in range(rng.randint(0, 4)):
        m = place(g, o, rng.choice(MINIONS),
                  atk=rng.randint(0, 6), hp=rng.randint(1, 9))
        m.taunt = rng.random() < 0.5
        m.divine_shield = rng.random() < 0.2
        m.stealth = rng.random() < 0.1
    for _ in range(rng.randint(0, 3)):
        give(g, p, rng.choice(BURN))
    return g, p


def kills(g, p, plan):
    """Run the plan on a clone; did the opponent hit 0?"""
    c = g.clone()
    cp = c.players[p.idx]
    execute(c, cp, plan)
    opp = cp.opponent
    return c.over or (opp.hp + opp.armor) <= 0


@pytest.mark.parametrize("deep", [False, True])
def test_no_reported_lethal_ever_fails_to_kill(carddb, deep):
    rng = random.Random(20260824 + int(deep))
    found = exact = approx = bad = 0
    failures = []
    for _ in range(ROUNDS):
        g, p = random_position(rng)
        plan = find_lethal(g, p, deep=deep)
        if plan is None:
            continue
        found += 1
        if getattr(plan, "approx", False):
            approx += 1
        else:
            exact += 1
        if not kills(g, p, plan):
            bad += 1
            if len(failures) < 3:
                failures.append(
                    (plan.describe(), getattr(plan, "approx", None),
                     p.opponent.hp + p.opponent.armor))
    assert found > 150, f"only {found} lethals in {ROUNDS} positions"
    assert bad == 0, (
        f"{bad}/{found} reported lethals did not kill "
        f"(exact={exact} approx={approx}): {failures}")


def test_the_solver_never_mutates_the_position_it_was_asked_about(carddb):
    rng = random.Random(7)
    for _ in range(60):
        g, p = random_position(rng)
        o = p.opponent
        before = (o.hp, o.armor, p.mana, len(p.hand), len(p.board),
                  len(o.board), [m.damage for m in p.board],
                  [m.damage for m in o.board])
        find_lethal(g, p, deep=True)
        after = (o.hp, o.armor, p.mana, len(p.hand), len(p.board),
                 len(o.board), [m.damage for m in p.board],
                 [m.damage for m in o.board])
        assert before == after


def test_deep_finds_at_least_as_much_as_shallow(carddb):
    """The extra ply may only add lethals, never remove them."""
    rng = random.Random(99)
    shallow_only = 0
    for _ in range(200):
        g, p = random_position(rng)
        a = find_lethal(g, p, deep=False)
        b = find_lethal(g, p, deep=True)
        if a is not None and b is None:
            shallow_only += 1
    assert shallow_only == 0


def test_plan_type_is_always_a_plan(carddb):
    rng = random.Random(3)
    for _ in range(120):
        g, p = random_position(rng)
        for deep in (False, True):
            plan = find_lethal(g, p, deep=deep)
            if plan is not None:
                assert isinstance(plan, Plan)
                assert isinstance(plan.approx, bool)
                assert plan.describe()


# Recall is deliberately not asserted here. Measuring it needs positions
# labelled "lethal existed" by something other than this solver, which is
# the lethal puzzle set the design schedules for v1 (§3.8). What *is*
# assertable today — and is asserted above — is that precision is 100% on
# generated positions and that the extra ply is monotone.
