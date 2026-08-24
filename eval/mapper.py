"""VisibleState -> a `hs2.Game` that is safe **for lethal only**.

This is deliberately narrow.  Hydrating a rules-complete `Game` from a
mid-match log is false if all we check is "are the visible card ids
implemented": battlecry listeners have already fired, deathrattles and
Dark Gifts were attached, Prepare set `locked_turn`, quests carry
progress, and `Minion.__init__` copies keywords off the `CardDef` without
replaying any in-play trigger registration.

So the overlay:

* calls `Game.__init__` and **nothing else** — no `start()`, no
  `_mulligan`, no start-of-game effects.  (`live.build_sim_state` calls
  `start()` and then hardcodes `turn = 10`; that is the bug this
  replaces.)
* gives the opponent 30 implemented fillers **of their own class**, not a
  copy of the user's deck — otherwise every read of the opponent's deck
  is a lie about a deck we have never seen.
* builds minions from the **log's tags**, not from `CardDef` keywords.
  An unimplemented card still has a truthful ATK/HP/TAUNT/WINDFURY in the
  log, and that is all `find_lethal` reads.

Two flags come out, and they answer different questions:

* `lethal_ok` — the hero/mana/board fields parsed, so a lethal the solver
  *finds* is real.  Unimplemented cards can only ever *add* damage, so
  they cannot manufacture a false positive.
* `hand_complete` — every card in our hand is implemented, so a lethal
  the solver *fails* to find is really absent.  Without it, "no lethal"
  is unknown rather than false, and the reviewer must stay silent.

`search_ok` is not set here and never will be by this module: a stats
overlay has no trigger graph.  Do not publish ΔWP from `apply` on it.
"""
from hs2.engine import (CardDef, HeroPowerState, Location, MINION, Minion,
                        Weapon)

# Log tag -> Minion attribute. Keywords come from the log so that an
# unimplemented card is still a faithful *combat* object.
_KEYWORD_TAGS = {
    "TAUNT": "taunt",
    "DIVINE_SHIELD": "divine_shield",
    "CHARGE": "charge",
    "RUSH": "rush",
    "WINDFURY": "windfury",
    "STEALTH": "stealth",
    "LIFESTEAL": "lifesteal",
    "POISONOUS": "poisonous",
    "ELUSIVE": "elusive",
    "REBORN": "reborn",
    "CANT_ATTACK": "cant_attack",
}

FILLER_PRIORITY = ("Wisp", "Sheep")


class Overlay:
    """The result of a mapping: the game, our player, and the flags."""

    __slots__ = ("game", "us", "them", "lethal_ok", "hand_complete",
                 "search_ok", "unimplemented", "reasons")

    def __init__(self, game, us, them):
        self.game = game
        self.us = us
        self.them = them
        self.lethal_ok = False
        self.hand_complete = False
        self.search_ok = False       # never set by a stats overlay
        self.unimplemented = []
        self.reasons = []

    def __repr__(self):
        return (f"<Overlay lethal_ok={self.lethal_ok} "
                f"hand_complete={self.hand_complete} "
                f"gap={len(self.unimplemented)}>")


def fields_parse(vs, us_pid=None):
    """Would the overlay's hero/mana fields come out of this state?

    This is `lethal_ok`'s real definition (design §2.5: "stats overlay
    safe for find_lethal") and it is a property of the *state*, not of
    whether we happened to run a search there.  `build_overlay` and the
    reviewer both go through here so the flag cannot mean two things.
    """
    us = vs.us if us_pid is None else us_pid
    them = 2 if us == 1 else 1
    for pid in (us, them):
        hero = vs.heroes.get(pid) or {}
        if hero.get("hp") is None:
            return False
        if not (vs.mana.get(pid) or {}):
            return False
    return True


def _filler_deck(cls, size=30):
    """30 implemented cards of `cls`, cheapest first, for a deck we never
    get to see. Never the user's own list: that would leak their deck into
    the opponent's."""
    from hs2 import carddata
    from hs2.decks import Deck
    if not carddata.DEFS:
        carddata.build_defs()
    pool = [d for d in carddata.DEFS.values()
            if d.implemented and d.coll and d.type == MINION
            and d.cls in (cls, "NEUTRAL") and not d.start_of_game
            and not d.quest and not d.secret]
    pool.sort(key=lambda d: (d.cls != cls, d.cost, d.name))
    ids = []
    for d in pool:
        ids.extend([d.id] * 2)
        if len(ids) >= size:
            break
    if not ids:
        for name in FILLER_PRIORITY:
            try:
                ids = [carddata.get_def(name).id] * size
                break
            except KeyError:
                continue
    return Deck(f"overlay-{cls}", cls, "midrange", ids[:size])


def _def_for(card_id, name=None):
    """The real CardDef, or a stub carrying just what combat needs."""
    from hs2 import carddata
    if card_id:
        try:
            return carddata.get_def(card_id), True
        except KeyError:
            pass
    if name:
        try:
            return carddata.get_def(name), True
        except KeyError:
            pass
    stub = CardDef(id=card_id or "unknown", name=name or card_id or "?",
                   type=MINION, cls="NEUTRAL", cost=0, atk=0, hp=1,
                   races=[], coll=False, implemented=False, text="")
    return stub, False


def _apply_tags(minion, view):
    tags = view.tags or {}
    for tag, attr in _KEYWORD_TAGS.items():
        if tag in tags:
            setattr(minion, attr, bool(tags[tag]))
    minion.atk_base = view.atk or 0
    minion.hp_base = view.health or 0
    minion.perm_atk = minion.perm_hp = minion.temp_atk = 0
    minion.aura_atk = minion.aura_hp = 0
    minion.damage = view.damage or 0
    minion.frozen = 1 if tags.get("FROZEN") else 0
    minion.dormant = int(tags.get("DORMANT") or 0)
    minion.silenced = bool(tags.get("SILENCED"))
    minion.attacks_done = int(tags.get("NUM_ATTACKS_THIS_TURN") or 0)
    # EXHAUSTED covers both summoning sickness and "already swung"; only
    # the first case is `just_summoned`.
    minion.just_summoned = bool(tags.get("EXHAUSTED")) and \
        minion.attacks_done == 0
    if tags.get("DIVINE_SHIELD") and tags.get("SHIELD_HITS"):
        minion.marks["shield_hits"] = int(tags["SHIELD_HITS"])
    # A stats overlay must not fire behaviour it never registered.
    minion.deathrattles = []
    minion.triggers = {}
    minion.aura = None
    return minion


def build_overlay(vs, us_cls=None, them_cls=None, us_pid=None):
    """Map one VisibleState onto a lethal-safe `Game`.

    Returns an `Overlay`. Never raises on a partial state: a field that
    did not parse turns a flag off instead.
    """
    us_pid = vs.us if us_pid is None else us_pid
    them_pid = 2 if us_pid == 1 else 1
    us_cls = _class_of(vs, us_pid, us_cls)
    them_cls = _class_of(vs, them_pid, them_cls)

    from hs2.engine import Game
    game = Game(_filler_deck(us_cls), _filler_deck(them_cls), seed=0,
                agents=None)          # __init__ only — no start(), no SOG
    ov = Overlay(game, game.players[0], game.players[1])
    by_pid = {us_pid: game.players[0], them_pid: game.players[1]}

    game.turn = int(vs.turn or 0)
    game.current = 0 if vs.current_player == us_pid else 1

    ok = fields_parse(vs, us_pid)
    for pid, p in by_pid.items():
        p.hand.clear()
        p.board.clear()
        p.deck = p.deck[:max(0, int(vs.deck_counts.get(pid, 0) or 0))]
        p.secrets = [s.get("card_id") or "?"
                     for s in vs.secrets.get(pid, [])
                     if isinstance(s, dict)]
        _apply_hero(p, vs.heroes.get(pid) or {})
        _apply_mana(p, vs.mana.get(pid) or {})
        _apply_weapon(game, p, vs.weapons.get(pid) or {})
        _apply_hero_power(game, p, vs.hero_powers.get(pid), ov)
        p.corpses = int(vs.corpses.get(pid, 0) or 0)
        _apply_board(game, p, vs.board(pid))

    gap = _apply_hand(game, by_pid[us_pid], vs.hand(us_pid))
    # The opponent's hand is face-down: only its size is knowable, and
    # `find_lethal` never reads it, so it stays empty rather than invented.
    ov.unimplemented = sorted(set(gap) | set(vs.implemented_gap or []))
    ov.hand_complete = not gap
    ov.lethal_ok = bool(ok)
    if not ok:
        ov.reasons.append("hero/mana fields did not parse")
    if gap:
        ov.reasons.append(
            "unimplemented cards in hand: " + ", ".join(sorted(set(gap))))
    return ov


def _class_of(vs, pid, given):
    if given:
        return str(given).upper()
    hero = vs.heroes.get(pid) or {}
    for key in ("cls", "class", "CLASS"):
        if hero.get(key):
            return str(hero[key]).upper()
    return "NEUTRAL"


def _apply_hero(p, hero):
    if not hero or hero.get("hp") is None:
        return False
    p.hp = int(hero.get("hp") or 0)
    p.max_hp = max(30, p.hp)
    p.armor = int(hero.get("armor") or 0)
    p.hero_attacks = int(hero.get("attacks") or 0)
    p.hero_frozen = 1 if hero.get("frozen") else 0
    p.immune = bool(hero.get("immune"))
    # Hero attack in the log already includes the weapon; `hero_attack`
    # re-adds it, so bank only the difference as temp attack.
    atk = int(hero.get("atk") or 0)
    p.marks["log_hero_atk"] = atk
    return True


def _apply_mana(p, mana):
    if not mana:
        return False
    crystals = int(mana.get("crystals") or 0)
    used = int(mana.get("used") or 0)
    temp = int(mana.get("temp") or 0)
    over = int(mana.get("overload") or 0)
    p.crystals = crystals
    p.temp_mana = temp
    p.overload_now = over
    p.mana = max(0, crystals - used - over + temp)
    return True


def _apply_weapon(game, p, weapon):
    if not weapon or not weapon.get("atk"):
        p.weapon = None
    else:
        card, _known = _def_for(weapon.get("card_id"), weapon.get("name"))
        w = Weapon(card)
        w.atk = int(weapon.get("atk") or 0)
        w.dur = int(weapon.get("dur") or 1)
        w.windfury = bool(weapon.get("windfury"))
        w.lifesteal = bool(weapon.get("lifesteal"))
        w.deathrattle = None
        w.triggers = {}
        p.weapon = game.reg(w)
    banked = p.marks.pop("log_hero_atk", None)
    if banked is not None:
        p.temp_atk = max(0, banked - (p.weapon.atk if p.weapon else 0))


def _apply_hero_power(game, p, info, ov=None):
    """Project the logged hero power, including whether it is spent.

    Two things the class default gets wrong. It is the wrong *card* when
    the power was replaced (Justicar, a hero card) — one real fixture has
    a Priest holding `Blessing of the Moon`, not `Lesser Heal`. And it is
    always shown unspent, so `find_lethal` counted a Fireblast the player
    had already fired: on the two real fixtures that is true of 22/56 and
    119/221 positions.

    When the log tells us nothing, the power is marked **used**. A hero
    power can only ever add damage, so assuming it is spent can cost us a
    lethal we would have found — while assuming it is free invents one,
    and a false missed-lethal call is the failure the design calls
    product-ending.
    """
    if p.hero_power is None:
        return None
    if not info:
        p.hero_power.used = p.hero_power.uses_per_turn
        if ov is not None and "hero power state unknown" not in ov.reasons:
            ov.reasons.append("hero power state unknown — treated as used")
        return p.hero_power

    card, known = _def_for(info.get("card_id"), info.get("name"))
    if known and card.type == "HERO_POWER":
        hp = HeroPowerState(card, cost=info.get("cost"))
        hp.eid = 0
        p.hero_power = game.reg(hp)
    elif info.get("cost") is not None:
        p.hero_power.cost = int(info["cost"])
    p.hero_power.used = 1 if info.get("exhausted") else 0
    if info.get("passive"):
        p.hero_power.passive = True
    p.hero_power2 = None
    return p.hero_power


def _apply_board(game, p, views):
    for v in views:
        tags = v.tags or {}
        kind = tags.get("CARDTYPE")
        card, _known = _def_for(v.card_id, v.name)
        if kind == "LOCATION" or (kind is None and card.type == "LOCATION"):
            loc = Location(card, p)
            loc.dur = int(v.health or v.tags.get("DURABILITY") or 1)
            loc.cooldown = 1 if tags.get("EXHAUSTED") else 0
            loc.use_fn = None
            loc.deathrattles = []
            p.board.append(game.reg(loc))
            continue
        p.board.append(game.reg(_apply_tags(Minion(card, p), v)))


def _apply_hand(game, p, views):
    """Our hand, as real CardInsts. Returns the unimplemented card names.

    An unimplemented card is dropped rather than stubbed: a stub with no
    `spell` would be silently unplayable anyway, and leaving it out keeps
    `find_lethal`'s knapsack honest about what it actually counted.
    """
    from hs2 import carddata
    gap = []
    for v in views:
        if not v.card_id and not v.name:
            continue          # a card we cannot see; nothing to overlay
        card, known = _def_for(v.card_id, v.name)
        if not known or not card.implemented:
            gap.append(v.name or v.card_id)
            continue
        inst = carddata.make_inst_any(card.id)
        tags = v.tags or {}
        if v.cost is not None and v.cost != card.cost:
            inst.cost_delta = int(v.cost) - card.cost
        if tags.get("LOCKED_TURN") or tags.get("PREPARED"):
            inst.locked_turn = game.turn
        p.hand.append(game.reg(inst))
    return gap


def refresh_hero_power(game, p, used=0, cost=None):
    """Hero-power state the log knows about (`used` from EXHAUSTED)."""
    hp = p.hero_power
    if hp is None:
        return None
    hp.used = int(used or 0)
    if cost is not None:
        hp.cost = int(cost)
    return hp


def hero_power_for_class(cls):
    from hs2 import carddata
    try:
        return HeroPowerState(carddata.hero_power_for(cls.upper()))
    except (KeyError, AttributeError):
        return None
