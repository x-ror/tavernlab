"""Enumerable, eid-addressed actions (PR 1 + PR 2).

`id()` is deliberately absent: it is not stable across `Game.clone()`, so an
Action built against the original would silently address nothing on the copy.
Every field here is a **stable eid** (`Game.reg` / `Game.by_eid`), which also
lets a log-derived decision and an enumerated action be compared directly.

Hero targets use the *player* eid (1 or 2).
"""
from dataclasses import dataclass, asdict
from typing import Literal, Optional

from .engine import (Minion, Location, Player, MINION, SPELL, LOCATION,
                     MAX_BOARD)

KINDS = ("play", "attack", "hero_attack", "hero_power", "location",
         "prepare", "end_turn", "discover", "mulligan", "choose")


@dataclass(frozen=True)
class Action:
    kind: Literal["play", "attack", "hero_attack", "hero_power",
                  "location", "prepare", "end_turn",
                  "discover", "mulligan", "choose"]
    eid: Optional[int] = None           # card / minion / location being used
    attacker_eid: Optional[int] = None  # minion eid; None + kind=hero_attack
    target_eid: Optional[int] = None    # entity eid; hero target = 1 | 2
    choice: Optional[int] = None        # choose-one index
    position: Optional[int] = None
    picks: Optional[tuple] = None       # mulligan keep eids / discover pick

    def to_dict(self):
        return {k: v for k, v in asdict(self).items() if v is not None}

    @staticmethod
    def from_dict(d):
        picks = d.get("picks")
        return Action(kind=d["kind"], eid=d.get("eid"),
                      attacker_eid=d.get("attacker_eid"),
                      target_eid=d.get("target_eid"),
                      choice=d.get("choice"), position=d.get("position"),
                      picks=tuple(picks) if picks is not None else None)


# ---------------------------------------------------------------- targets
def eid_of(entity):
    if entity is None:
        return None
    return entity.eid


def target_kind(card, for_battlecry):
    """Which target set the engine will feed this card."""
    if for_battlecry:
        return card.battlecry_target or card.target
    return card.target


def targets_for(game, p, kind, spell_like=False):
    """Legal targets for `kind`; `[None]` when the card takes no target.

    `spell_like` applies the Elusive rule ("can't be targeted by spells or
    Hero Powers").  The engine itself does not enforce Elusive, so this
    enumerator is the one place the rule lives — an intentional asymmetry,
    not an oversight.
    """
    if not kind:
        return [None]
    opp = p.opponent

    def ok(m):
        if m.dead or m.dormant:
            return False
        return not (spell_like and m.elusive)

    mine = [m for m in p.active_minions if ok(m)]
    theirs = [m for m in opp.active_minions if ok(m) and not m.stealth]
    if kind == "any":
        return [p, opp] + mine + theirs
    if kind == "minion":
        return mine + theirs
    if kind == "friendly_minion":
        return mine
    if kind == "enemy_minion":
        return theirs
    if kind == "damaged_enemy_minion":
        return [m for m in theirs if m.damage > 0]
    if kind == "enemy":
        return [opp] + theirs
    return [None]


_ADJACENT_WORDS = ("adjacent", "next to", "to the left", "to the right",
                   "neighbo")


def _positions(p, card):
    """Board slots worth enumerating. Position only changes the outcome for
    adjacency cards, so everything else gets the default append slot."""
    if card.type not in (MINION, LOCATION):
        return (None,)
    text = (card.text or "").lower()
    if not any(w in text for w in _ADJACENT_WORDS):
        return (None,)
    return tuple(range(len(p.board) + 1))


def attack_targets(p, face_ok):
    """Taunt is enforced here; `Game.attack` does not enforce it."""
    opp = p.opponent
    taunts = [m for m in opp.active_minions
              if m.taunt and not m.stealth and not m.dead]
    if taunts:
        return taunts
    out = [m for m in opp.active_minions if not m.dead and not m.stealth]
    return out + [opp] if face_ok else out


# ---------------------------------------------------------- enumeration
def legal_actions(game, p):
    """Returns `(actions, actions_complete)`.

    `actions_complete` is False when the enumeration provably does not cover
    every legal move — today that means an **unimplemented card sits in
    hand**, so the engine could not play it even though the human could.
    Nothing downstream may publish a skill label while it is False
    (design §3.3).

    choose-one cards produce one `play` Action per choice index.
    `kind="discover"` is deliberately NOT enumerated: the offered set is
    hidden information, so a discover is a *logged* decision, not a
    searchable one.

    A card whose `target` kind has no legal target yields **no** Action:
    Hearthstone will not let you cast it either, so omitting it does not
    make the set incomplete.
    """
    if game.over:
        return [], True
    game._ensure_eids()
    out = []
    complete = True

    for inst in p.hand:
        card = inst.card
        if not card.implemented:
            complete = False
            continue
        if inst.locked_turn == game.turn:
            continue
        if p.effective_cost(inst) > p.mana:
            continue
        if card.corpse_cost and p.corpses < card.corpse_cost:
            continue
        if card.type in (MINION, LOCATION) and len(p.board) >= MAX_BOARD:
            continue
        if card.play_if and not card.play_if(game, p):
            continue
        choices = (tuple(range(len(card.choose))) if card.choose
                   else (None,))
        kind = target_kind(card, card.type != SPELL)
        tgts = targets_for(game, p, kind, spell_like=card.type == SPELL)
        for ch in choices:
            for t in tgts:
                for pos in _positions(p, card):
                    out.append(Action("play", eid=inst.eid,
                                      target_eid=eid_of(t),
                                      choice=ch, position=pos))

    for m in p.active_minions:
        if not m.can_attack():
            continue
        for t in attack_targets(p, m.can_attack_face()):
            out.append(Action("attack", attacker_eid=m.eid,
                              target_eid=eid_of(t)))
    if p.hero_can_attack():
        for t in attack_targets(p, True):
            out.append(Action("hero_attack", target_eid=eid_of(t)))

    for which in (p.hero_power, p.hero_power2):
        if which is None or which.passive:
            continue
        if which.used >= which.uses_per_turn:
            continue
        if which.corpse_cost and p.corpses < which.corpse_cost:
            continue
        if p.mana < which.cost:
            continue
        for t in targets_for(game, p, which.card.target, spell_like=True):
            out.append(Action("hero_power", eid=which.eid,
                              target_eid=eid_of(t)))

    for loc in p.locations:
        if not loc.usable():
            continue
        for t in targets_for(game, p, loc.card.target):
            out.append(Action("location", eid=loc.eid,
                              target_eid=eid_of(t)))

    if p.mana > 0:
        for inst in p.hand:
            if inst.card.prepare and inst.locked_turn != game.turn:
                out.append(Action("prepare", eid=inst.eid))

    out.append(Action("end_turn"))
    return out, complete


# ------------------------------------------------------------------ apply
def apply(game, p, action, forced_picks=None):
    """Execute one Action.

    `forced_picks` is a queue of eids / card ids / indices consumed by
    nested `Game.discover` calls (including discovers fired from inside a
    battlecry).  When it runs dry the agent picks again and the line stops
    being a faithful replay — callers that care must check
    `game._forced_picks` afterwards.
    """
    game._ensure_eids()
    prev = game._forced_picks
    game._forced_picks = list(forced_picks) if forced_picks else []
    try:
        return _dispatch(game, p, action)
    finally:
        game._forced_picks = prev


def _dispatch(game, p, a):
    tgt = game.by_eid(a.target_eid)
    if a.kind == "play":
        inst = game.by_eid(a.eid)
        if inst is None or inst not in p.hand:
            return False
        return game.play_card(p, inst, target=tgt, choice=a.choice,
                              position=a.position)
    if a.kind == "attack":
        m = game.by_eid(a.attacker_eid)
        if m is None or m not in p.board or not m.can_attack():
            return False
        game.attack(p, m, tgt)
        return True
    if a.kind == "hero_attack":
        if not p.hero_can_attack():
            return False
        game.attack(p, "hero", tgt)
        return True
    if a.kind == "hero_power":
        which = game.by_eid(a.eid) if a.eid else None
        return game.use_hero_power(p, tgt, which=which)
    if a.kind == "location":
        loc = game.by_eid(a.eid)
        if not isinstance(loc, Location) or loc not in p.board:
            return False
        return game.use_location(p, loc, tgt)
    if a.kind == "prepare":
        inst = game.by_eid(a.eid)
        if inst is None or inst not in p.hand:
            return False
        return game.prepare_card(p, inst)
    if a.kind == "end_turn":
        game.end_turn(p)
        return True
    if a.kind in ("discover", "choose", "mulligan"):
        # Logged decisions: replayed through forced_picks / keep_fns, not
        # executed standalone.
        return False
    raise ValueError(f"unknown action kind: {a.kind!r}")


def advance_turn(game):
    """Driver helper: hand the turn over the way `Game.run` does."""
    game.current = 1 - game.current
    game.turn += 1
    game.begin_turn(game.players[game.current])
