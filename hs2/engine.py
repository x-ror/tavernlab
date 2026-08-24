"""Hearthstone engine v2 — Standard (2026) rules.

Extends the classic rule set with: Death Knight (Corpses), Locations,
Dormant, Discover, Quests/Sidequests, Start-of-Game effects, Herald
counters + class Soldiers, Colossal appendages, spell schools, Kindred
tracking, Prepare, per-card-instance cost modification, temporary mana,
delayed/scheduled effects, hero cards, replaced hero powers, The Void,
temporary cards, Elusive, Reborn, Tradeable.
"""
import random

from .effects import HANDLERS, PendingListen, PendingTurnStart

MINION, SPELL, WEAPON, LOCATION, HERO, HERO_POWER = \
    "MINION", "SPELL", "WEAPON", "LOCATION", "HERO", "HERO_POWER"
MAX_BOARD = 7
MAX_HAND = 10
MAX_MANA = 10
TURN_LIMIT = 89

CLASSES = ["DEATHKNIGHT", "DEMONHUNTER", "DRUID", "HUNTER", "MAGE",
           "PALADIN", "PRIEST", "ROGUE", "SHAMAN", "WARLOCK", "WARRIOR"]


class CardDef:
    """Static definition: official data merged with behavior overlay."""
    __slots__ = (
        "id", "dbf", "name", "type", "cls", "cost", "atk", "hp", "dur",
        "armor", "races", "school", "text", "coll", "rarity", "set",
        # keywords
        "taunt", "divine_shield", "charge", "rush", "windfury", "stealth",
        "lifesteal", "poisonous", "elusive", "reborn", "cant_attack",
        "spell_dmg", "dormant", "tradeable", "prepare", "outcast_discount",
        "corpse_cost", "overload",
        # behaviors
        "battlecry", "deathrattle", "triggers", "aura", "spell", "choose",
        "combo", "combo_spell", "target", "battlecry_target", "cost_fn",
        "start_of_game", "location_use", "colossal", "quest", "secret",
        "kindred", "on_summon_fx", "hero_power_use", "enrage_atk",
        "ai_hint", "play_if", "implemented", "notes",
    )

    def __init__(self, **kw):
        for s in self.__slots__:
            setattr(self, s, kw.get(s))
        self.triggers = self.triggers or {}
        self.races = self.races or []
        self.colossal = self.colossal or []
        self.ai_hint = self.ai_hint or ()
        for flag in ("taunt", "divine_shield", "charge", "rush", "windfury",
                     "stealth", "lifesteal", "poisonous", "elusive",
                     "reborn", "cant_attack", "tradeable", "prepare",
                     "coll", "implemented"):
            if getattr(self, flag) is None:
                setattr(self, flag, False)
        for num in ("cost", "atk", "hp", "dur", "armor", "spell_dmg",
                    "dormant", "outcast_discount", "corpse_cost",
                    "overload", "enrage_atk"):
            if getattr(self, num) is None:
                setattr(self, num, 0)


class CardInst:
    """A card in hand or deck: definition + per-copy state."""
    __slots__ = ("card", "cost_delta", "temporary", "locked_turn", "marks",
                 "gift", "eid")

    def __init__(self, card):
        self.eid = 0
        self.card = card
        self.cost_delta = 0
        self.temporary = False
        self.locked_turn = -1   # Prepare: unplayable during this turn no.
        self.marks = None       # lazy dict
        self.gift = None        # Dark Gift applied in hand

    def mark(self):
        if self.marks is None:
            self.marks = {}
        return self.marks

    @property
    def name(self):
        return self.card.name


class Minion:
    __slots__ = (
        "card", "owner", "atk_base", "hp_base", "perm_atk", "perm_hp",
        "temp_atk", "aura_atk", "aura_hp", "damage",
        "taunt", "divine_shield", "charge", "rush", "windfury", "stealth",
        "lifesteal", "poisonous", "elusive", "reborn", "cant_attack",
        "spell_dmg", "frozen", "silenced", "attacks_done", "just_summoned",
        "deathrattles", "triggers", "aura", "enrage_atk", "pending_destroy",
        "to_die_eot", "dormant", "marks", "immune_attacking", "eid",
    )

    def __init__(self, card, owner):
        self.eid = 0
        self.card = card
        self.owner = owner
        self.atk_base = card.atk
        self.hp_base = card.hp
        self.perm_atk = self.perm_hp = self.temp_atk = 0
        self.aura_atk = self.aura_hp = 0
        self.damage = 0
        for f in ("taunt", "divine_shield", "charge", "rush", "windfury",
                  "stealth", "lifesteal", "poisonous", "elusive", "reborn",
                  "cant_attack"):
            setattr(self, f, getattr(card, f))
        self.spell_dmg = card.spell_dmg
        self.frozen = 0
        self.silenced = False
        self.attacks_done = 0
        self.just_summoned = True
        self.deathrattles = [card.deathrattle] if card.deathrattle else []
        self.triggers = dict(card.triggers)
        self.aura = card.aura
        self.enrage_atk = card.enrage_atk
        self.pending_destroy = False
        self.to_die_eot = False
        self.dormant = card.dormant
        self.marks = {}
        self.immune_attacking = False

    @property
    def races(self):
        return self.card.races

    @property
    def attack(self):
        atk = self.atk_base + self.perm_atk + self.temp_atk + self.aura_atk
        if self.enrage_atk and self.damage > 0 and not self.silenced:
            atk += self.enrage_atk
        return max(0, atk)

    @property
    def max_hp(self):
        return self.hp_base + self.perm_hp + self.aura_hp

    @property
    def health(self):
        return self.max_hp - self.damage

    @property
    def dead(self):
        return (self.health <= 0 or self.pending_destroy) and not self.dormant

    def can_attack(self):
        if self.dormant or self.cant_attack or self.frozen or self.attack <= 0:
            return False
        if self.attacks_done >= (2 if self.windfury else 1):
            return False
        if self.just_summoned and not (self.charge or self.rush):
            return False
        return True

    def can_attack_face(self):
        return self.can_attack() and not (self.just_summoned and self.rush
                                          and not self.charge)

    def silence(self):
        self.silenced = True
        for f in ("taunt", "divine_shield", "windfury", "stealth",
                  "lifesteal", "poisonous", "elusive", "reborn",
                  "cant_attack"):
            setattr(self, f, False)
        self.spell_dmg = 0
        self.perm_atk = min(self.perm_atk, 0)
        self.perm_hp = min(self.perm_hp, 0)
        self.temp_atk = 0
        self.frozen = 0
        self.deathrattles = []
        self.triggers = {}
        self.aura = None
        self.enrage_atk = 0


class Weapon:
    __slots__ = ("card", "atk", "dur", "windfury", "lifesteal",
                 "deathrattle", "triggers", "marks", "eid")

    def __init__(self, card):
        self.eid = 0
        self.card = card
        self.atk = card.atk
        self.dur = card.dur
        self.windfury = card.windfury
        self.lifesteal = card.lifesteal
        self.deathrattle = card.deathrattle
        self.triggers = dict(card.triggers)
        self.marks = {}


class Location:
    __slots__ = ("card", "owner", "dur", "cooldown", "use_fn", "marks",
                 "deathrattles", "eid")

    def __init__(self, card, owner):
        self.eid = 0
        self.card = card
        self.owner = owner
        self.dur = card.dur
        self.cooldown = 0
        self.use_fn = card.location_use
        self.marks = {}
        self.deathrattles = [card.deathrattle] if card.deathrattle else []

    def usable(self):
        return self.dur > 0 and self.cooldown == 0


class HeroPowerState:
    __slots__ = ("card", "cost", "uses_per_turn", "used", "use_fn",
                 "corpse_cost", "passive", "marks", "eid")

    def __init__(self, card, cost=None, corpse_cost=0):
        self.eid = 0
        self.card = card
        self.cost = card.cost if cost is None else cost
        self.uses_per_turn = 1
        self.used = 0
        self.use_fn = card.hero_power_use
        self.corpse_cost = corpse_cost
        self.passive = "Passive" in (card.text or "")
        self.marks = {}


class Player:
    __slots__ = (
        "game", "idx", "cls", "hp", "max_hp", "armor", "temp_atk", "weapon",
        "hand", "deck", "board", "secrets", "mana", "crystals", "temp_mana",
        "overload_next", "overload_now", "fatigue", "hero_attacks",
        "hero_frozen", "cards_played_turn", "spell_discount", "archetype",
        "immune", "hero_attacked_turn", "hero_attacks_game", "corpses",
        "herald", "quest", "sidequest", "void", "hero_power",
        "hero_power2", "listeners", "turn_start_fx", "played_types_turn",
        "played_types_last", "played_cards_turn", "minions_died_turn",
        "minions_died_game", "dead_minion_cards", "spells_played_game",
        "spell_schools_turn", "spell_schools_last", "next_outcast_discount",
        "spell_cost_penalty", "graveyard_dr", "marks", "eid",
    )

    def __init__(self, game, idx, cls, deck_insts, archetype, hero_power):
        self.game = game
        self.idx = idx
        self.eid = idx + 1        # controller eid: 1 | 2
        self.cls = cls
        self.hp = 30
        self.max_hp = 30
        self.armor = 0
        self.temp_atk = 0
        self.weapon = None
        self.hand = []
        self.deck = list(deck_insts)
        self.board = []          # Minions and Locations
        self.secrets = []
        self.mana = 0
        self.crystals = 0
        self.temp_mana = 0
        self.overload_next = 0
        self.overload_now = 0
        self.fatigue = 0
        self.hero_attacks = 0
        self.hero_frozen = 0
        self.cards_played_turn = 0
        self.spell_discount = 0
        self.archetype = archetype
        self.immune = False
        self.hero_attacked_turn = False
        self.hero_attacks_game = 0
        self.corpses = 0
        self.herald = 0
        self.quest = None
        self.sidequest = None
        self.void = []
        self.hero_power = hero_power
        self.hero_power2 = None
        self.listeners = []      # list[PendingListen]
        self.turn_start_fx = []  # list[PendingTurnStart]
        self.played_types_turn = set()
        self.played_types_last = set()
        self.played_cards_turn = []
        self.minions_died_turn = 0
        self.minions_died_game = 0
        self.dead_minion_cards = []
        self.spells_played_game = 0
        self.spell_schools_turn = set()
        self.spell_schools_last = set()
        self.next_outcast_discount = 0
        self.spell_cost_penalty = 0   # Cult Neophyte (on spells this turn)
        self.graveyard_dr = []        # deathrattle minions died (Umbra相)
        self.marks = {}

    @property
    def opponent(self):
        return self.game.players[1 - self.idx]

    @property
    def minions(self):
        return [m for m in self.board if isinstance(m, Minion)]

    @property
    def active_minions(self):
        return [m for m in self.board
                if isinstance(m, Minion) and not m.dormant]

    @property
    def locations(self):
        return [l for l in self.board if isinstance(l, Location)]

    @property
    def hero_attack(self):
        atk = self.temp_atk
        if self.weapon:
            atk += self.weapon.atk
        for m in self.active_minions:
            fx = m.card.on_summon_fx
            if fx == "hero_atk_aura" and not m.silenced:
                atk += m.marks.get("hero_atk", 0)
        return atk

    @property
    def spell_power(self):
        return sum(m.spell_dmg for m in self.active_minions)

    def hero_can_attack(self):
        if self.hero_frozen or self.hero_attack <= 0:
            return False
        max_att = 2 if (self.weapon and self.weapon.windfury) else 1
        return self.hero_attacks < max_att

    def effective_cost(self, inst):
        card = inst.card
        cost = card.cost + inst.cost_delta
        if card.cost_fn:
            cost = card.cost_fn(self.game, self, inst, cost)
        if card.type == SPELL:
            cost -= self.spell_discount
            cost += self.spell_cost_penalty
        if card.outcast_discount and self.is_outcast(inst):
            cost -= card.outcast_discount
        if card.text and "Outcast" in card.text and \
                self.next_outcast_discount and self.is_outcast(inst):
            pass
        # hand-cost auras from own board
        for m in self.active_minions:
            hf = m.card.on_summon_fx
            if hf == "hand_cost_aura" and not m.silenced:
                fn = m.card.triggers.get("hand_cost")
                if fn:
                    cost = fn(self.game, self, m, inst, cost)
        return max(0, cost)

    def is_outcast(self, inst):
        return self.hand and (self.hand[0] is inst or self.hand[-1] is inst)

    def listen(self, event, handler_id, source_eid=None, expiry_turn=None,
               **args):
        """Register a clone-safe pending listener.

        `handler_id` is a key in `hs2.effects.HANDLERS`; closures are
        forbidden because a clone would keep firing them on the original
        entities.  `args` must be JSON-serializable (eids, ints, strings).
        """
        self.listeners.append(PendingListen(
            event, handler_id,
            self.eid if source_eid is None else source_eid,
            expiry_turn, args))

    def at_turn_start(self, handler_id, source_eid=None, turns=1,
                      repeat=False, **args):
        self.turn_start_fx.append(PendingTurnStart(
            handler_id, self.eid if source_eid is None else source_eid,
            turns, repeat, args))


class Game:
    def __init__(self, deck0, deck1, seed=None, agents=None):
        from .carddata import make_inst, hero_power_for
        self.rng = random.Random(seed)
        self.agents = agents
        self.turn = 0
        self.current = 0
        self.over = False
        self.winner = None
        self._outcast = False
        self._current_inst = None
        self._forced_picks = None      # queue consumed by Game.discover
        self._last_discover = None     # (offered, chosen) of the last one
        self._by_eid = {}              # eid -> entity (the entity universe)
        self._next_eid = 3             # 1 and 2 are the players
        self.players = []
        for i, d in enumerate((deck0, deck1)):
            insts = [make_inst(cid) for cid in d.card_ids]
            hp = HeroPowerState(hero_power_for(d.cls))
            self.players.append(Player(self, i, d.cls, insts, d.archetype,
                                       hp))
        for p in self.players:
            self._by_eid[p.eid] = p
            self.reg(p.hero_power)
            for inst in p.deck:
                self.reg(inst)

    # ------------------------------------------------------------- entities
    def reg(self, obj):
        """Give `obj` a stable eid and put it in the entity universe."""
        if obj is None:
            return None
        if obj.eid:
            self._by_eid.setdefault(obj.eid, obj)
            return obj
        obj.eid = self._next_eid
        self._next_eid += 1
        self._by_eid[obj.eid] = obj
        return obj

    def by_eid(self, eid):
        """Entity for `eid`; player eids are 1 and 2."""
        if eid is None:
            return None
        return self._by_eid.get(eid)

    def _ensure_eids(self):
        """Sweep every reachable entity slot and register stragglers.

        `impls` may build a `CardInst` and drop it straight into a deck or a
        marks list, so eager registration in the engine is not sufficient.
        Clone / legal_actions / apply all call this first.
        """
        for p in self.players:
            self._by_eid[p.eid] = p
            for zone in (p.hand, p.deck, p.board, p.void):
                for o in zone:
                    self.reg(o)
            self.reg(p.weapon)
            self.reg(p.hero_power)
            self.reg(p.hero_power2)
            for v in list(p.marks.values()):
                _sweep_marks(self, v)
            for m in p.board:
                if isinstance(m, (Minion, Location)):
                    for v in list(m.marks.values()):
                        _sweep_marks(self, v)

    # ------------------------------------------------------------------ setup
    def start(self, first=0, keep_fns=None):
        """`keep_fns`: optional (fn0, fn1); each `fn(player, drawn) -> keep`.

        Used by review/mulligan-lab to replay the hand the human actually
        kept instead of the scripted cost threshold.
        """
        self.current = first
        for p in self.players:
            self.rng.shuffle(p.deck)
        for p in self.players:
            p.marks["start_costs"] = len({i.card.cost for i in p.deck})
            p.marks["start_deck"] = len(p.deck)
        # start-of-game effects (before mulligan)
        for p in self.players:
            for inst in list(p.deck):
                if inst.card.start_of_game:
                    inst.card.start_of_game(self, p, inst)
        first_p, second_p = self.players[first], self.players[1 - first]
        kf = keep_fns or (None, None)
        self._mulligan(first_p, 3, kf[first_p.idx])
        self._mulligan(second_p, 4, kf[second_p.idx])
        from .carddata import make_inst_by_name
        second_p.hand.append(self.reg(make_inst_by_name("The Coin")))
        for p in self.players:
            p.marks["start_hand"] = [i.card for i in p.hand]

    def _mulligan(self, p, n, keep_fn=None):
        drawn = [p.deck.pop() for _ in range(min(n, len(p.deck)))]
        if keep_fn is not None:
            wanted = keep_fn(p, drawn)
            keep = [c for c in drawn if c in wanted]
        else:
            thresh = 2 if p.archetype == "aggro" else 3
            keep = [c for c in drawn if c.card.cost <= thresh]
        kept = set(id(c) for c in keep)
        back = [c for c in drawn if id(c) not in kept]
        p.deck.extend(back)
        self.rng.shuffle(p.deck)
        while len(keep) < n and p.deck:
            keep.append(p.deck.pop())
        p.hand.extend(keep)

    # ------------------------------------------------------------------- flow
    def run(self):
        self.start(first=self.rng.randint(0, 1))
        while not self.over:
            self.turn += 1
            if self.turn > TURN_LIMIT:
                self.winner = None
                self.over = True
                break
            p = self.players[self.current]
            self.begin_turn(p)
            if not self.over:
                self.agents[p.idx].take_turn(self, p)
            if not self.over:
                self.end_turn(p)
            self.current = 1 - self.current
        return self.winner

    def begin_turn(self, p):
        if p.crystals < MAX_MANA:
            p.crystals += 1
        p.overload_now = p.overload_next
        p.overload_next = 0
        p.temp_mana = 0
        p.mana = max(0, p.crystals - p.overload_now)
        if p.hero_power:
            p.hero_power.used = 0
        if p.hero_power2:
            p.hero_power2.used = 0
        p.hero_attacks = 0
        p.hero_attacked_turn = False
        p.cards_played_turn = 0
        p.temp_atk = 0
        p.played_types_last = p.played_types_turn
        p.played_types_turn = set()
        p.spell_schools_last = p.spell_schools_turn
        p.spell_schools_turn = set()
        p.played_cards_turn = []
        p.minions_died_turn = 0
        p.spell_cost_penalty = 0
        p.marks["spell_dmg_turn"] = 0
        for m in p.minions:
            m.attacks_done = 0
            m.just_summoned = False
            if m.dormant:
                m.dormant -= 1
                if m.dormant == 0:
                    m.just_summoned = True
                    fn = m.triggers.get("awaken")
                    if fn:
                        fn(self, p, m)
        for l in p.locations:
            if l.cooldown:
                l.cooldown -= 1
        # scheduled turn-start effects
        fx = p.turn_start_fx
        p.turn_start_fx = []
        for entry in fx:
            fn = HANDLERS.get(entry.handler_id)
            if fn is not None:
                fn(self, p, self.by_eid(entry.source_eid), **entry.args)
            if self.over:
                return
            if entry.repeat and entry.turns_left - 1 > 0:
                nxt = entry.copy()
                nxt.turns_left -= 1
                p.turn_start_fx.append(nxt)
        # Void: Irida — handled via scheduled fx
        self.draw(p)
        if self.over:
            return
        self.fire("turn_start", p)
        self.check_deaths()

    def end_turn(self, p):
        self.fire("turn_end", p)
        self.check_deaths()
        p.temp_atk = 0
        p.spell_discount = 0
        p.opponent.spell_cost_penalty = 0
        for m in list(p.minions):
            m.temp_atk = 0
            m.immune_attacking = False
            if m.to_die_eot or m.card.name == "Frail Ghoul":
                m.pending_destroy = True
        # temporary cards vanish
        for zone in (p.hand,):
            for inst in list(zone):
                if inst.temporary:
                    zone.remove(inst)
        for m in p.minions:
            if m.frozen:
                m.frozen -= 1
        if p.hero_frozen:
            p.hero_frozen -= 1
        # expired listeners
        for pl in self.players:
            pl.listeners = [l for l in pl.listeners
                            if l.expiry_turn is None
                            or l.expiry_turn > self.turn]
        self.check_deaths()
        self.recompute_auras()

    # ------------------------------------------------------------------ draws
    def draw(self, p, n=1, filt=None):
        last = None
        for _ in range(n):
            pool = p.deck
            if filt:
                cands = [c for c in p.deck if filt(c)]
                if not cands:
                    continue
                inst = cands[-1]
                p.deck.remove(inst)
            else:
                if not p.deck:
                    p.fatigue += 1
                    self.deal_damage(None, p, p.fatigue)
                    if self.over:
                        return None
                    continue
                inst = p.deck.pop()
            self.reg(inst)
            if len(p.hand) >= MAX_HAND:
                self.fire("overdraw", p, inst)
                continue
            p.hand.append(inst)
            mk = inst.mark()
            mk.setdefault("draw_spend", p.marks.get("mana_spent", 0))
            mk.setdefault("draw_turn", self.turn)
            mk.setdefault("draw_minions", p.marks.get("minions_played", 0))
            mk.setdefault("draw_copies", p.marks.get("opp_copies_played", 0))
            self.fire("card_drawn", p, inst)
            last = inst
        return last

    def add_to_hand(self, p, inst):
        self.reg(inst)
        if len(p.hand) < MAX_HAND:
            p.hand.append(inst)
            return inst
        return None

    def get_card(self, p, name_or_id, **state):
        from .carddata import make_inst_any
        inst = make_inst_any(name_or_id)
        for k, v in state.items():
            setattr(inst, k, v)
        return self.add_to_hand(p, inst)

    # ---------------------------------------------------------------- summons
    def summon(self, p, card_or_name, position=None, dormant=None):
        from .carddata import get_def
        if len(p.board) >= MAX_BOARD:
            return None
        card = card_or_name if isinstance(card_or_name, CardDef) \
            else get_def(card_or_name)
        m = self.reg(Minion(card, p))
        if dormant is not None:
            m.dormant = dormant
        if position is None:
            p.board.append(m)
        else:
            p.board.insert(position, m)
        # colossal appendages
        for app in card.colossal:
            if len(p.board) >= MAX_BOARD:
                break
            self.summon(p, app)
        self.recompute_auras()
        if card.on_summon_fx and callable(card.on_summon_fx):
            card.on_summon_fx(self, p, m)
        self.fire("summon", p, m)
        return m

    # ------------------------------------------------------------------ plays
    def play_card(self, p, inst, target=None, choice=None, position=None):
        card = inst.card
        cost = p.effective_cost(inst)
        if cost > p.mana or inst.locked_turn == self.turn:
            return False
        if card.corpse_cost:
            if p.corpses < card.corpse_cost:
                return False
            p.corpses -= card.corpse_cost
        outcast = p.is_outcast(inst)
        combo_active = p.cards_played_turn > 0
        self._current_inst = inst
        p.mana -= cost
        p.marks["mana_spent"] = p.marks.get("mana_spent", 0) + cost
        if card.type == SPELL and p.spell_discount:
            p.spell_discount = 0
        p.hand.remove(inst)
        p.cards_played_turn += 1
        p.played_cards_turn.append(card)
        if card.type == MINION:
            p.marks["minions_played"] = p.marks.get("minions_played", 0) + 1
            p.marks["last_minion"] = card
        if inst.marks and inst.marks.get("stolen"):
            p.marks["opp_copies_played"] = \
                p.marks.get("opp_copies_played", 0) + 1
        for r in card.races:
            p.played_types_turn.add(r)
        if card.school:
            p.played_types_turn.add(card.school)
        if card.overload:
            p.overload_next += card.overload
            self.fire("overload", p)
        self._outcast = outcast

        if card.type == MINION:
            if len(p.board) >= MAX_BOARD:
                return True
            m = self.reg(Minion(card, p))
            if inst.gift:
                inst.gift(self, p, m)
            if position is None:
                p.board.append(m)
            else:
                p.board.insert(position, m)
            for app in card.colossal:
                if len(p.board) < MAX_BOARD:
                    self.summon(p, app)
            self.recompute_auras()
            kindred_ok = card.kindred and any(
                r in p.played_types_last for r in card.races)
            bc = card.combo if (combo_active and card.combo) \
                else card.battlecry
            if card.choose:
                bc = card.choose[choice or 0]
            self.fire("pre_battlecry", p, m)
            if bc:
                n_casts = 2 if (m.marks.get("bc_twice") or
                                p.marks.get("bc_twice_all")) else 1
                for _ in range(n_casts):
                    bc(self, p, m, target)
                    if self.over:
                        return True
            if kindred_ok and not self.over:
                card.kindred(self, p, m, target)
            if not self.over:
                self.fire("minion_played", p, m)
            self.check_deaths()
        elif card.type == SPELL:
            opp = p.opponent
            if "Counterspell" in opp.secrets:
                self.pop_secret(opp, "Counterspell")
                return True
            p.spells_played_game += 1
            if card.school:
                p.spell_schools_turn.add(card.school)
            if card.secret:
                if card.secret not in p.secrets and len(p.secrets) < 5:
                    p.secrets.append(card.secret)
            elif card.quest:
                self._start_quest(p, card)
            else:
                fn = card.spell
                if combo_active and card.combo_spell:
                    fn = card.combo_spell
                if card.choose:
                    fn = card.choose[choice or 0]
                if target is not None and self._gone(target):
                    target = None
                if fn:
                    n_casts = 2 if (p.marks.get("cast_twice_other_cls") and
                                    card.cls not in (p.cls, "NEUTRAL")) else 1
                    for _ in range(n_casts):
                        fn(self, p, target)
                        if self.over:
                            return True
            self.fire("spell_cast", p, card)
            self.check_deaths()
        elif card.type == WEAPON:
            p.weapon = self.reg(Weapon(card))
            if card.battlecry:
                card.battlecry(self, p, None, target)
            self.check_deaths()
        elif card.type == LOCATION:
            if len(p.board) >= MAX_BOARD:
                return True
            loc = self.reg(Location(card, p))
            p.board.append(loc)
            self.check_deaths()
        elif card.type == HERO:
            p.armor += card.armor
            from .carddata import get_def
            if card.battlecry:
                card.battlecry(self, p, None, target)
            self.check_deaths()

        if not self.over:
            self.fire("card_played", p, card)
            self._quest_progress(p, "card_played", card)
        return True

    def _start_quest(self, p, card):
        q = dict(card.quest)
        q["card"] = card
        q["progress"] = 0
        if q.get("side"):
            p.sidequest = q
        else:
            p.quest = q

    def _quest_progress(self, p, event, *args):
        for q in (p.quest, p.sidequest):
            if not q or q.get("done"):
                continue
            inc = q["check"](self, p, event, *args)
            if inc:
                q["progress"] += inc
                if q["progress"] >= q["target"]:
                    q["done"] = True
                    q["reward"](self, p)

    def use_location(self, p, loc, target=None):
        if not loc.usable():
            return False
        loc.dur -= 1
        loc.cooldown = 2  # unusable next turn (ticks at own turn start)
        if loc.use_fn:
            loc.use_fn(self, p, loc, target)
        if loc.dur <= 0:
            p.board.remove(loc)
            for dr in loc.deathrattles:
                dr(self, p, loc)
        self.check_deaths()
        return True

    def prepare_card(self, p, inst):
        """Prepare keyword: bank remaining mana into a cost reduction."""
        if p.mana <= 0:
            return False
        inst.cost_delta -= (p.mana + 1)
        inst.locked_turn = self.turn
        p.mana = 0
        return True

    def _gone(self, target):
        if isinstance(target, Minion):
            return target.dead or target not in target.owner.board
        return False

    # ---------------------------------------------------------------- damage
    def deal_damage(self, source, target, amount, from_minion=None,
                    spell=False, _gorishi=False):
        if amount <= 0 or self.over:
            return 0
        cur = self.players[self.current]
        if (not _gorishi and amount == 2 and cur.marks.get("colossus")
                and (target is cur.opponent or
                     (isinstance(target, Minion)
                      and target.owner is cur.opponent))):
            self.deal_damage(source, target, 2, from_minion=from_minion,
                             spell=spell, _gorishi=True)
        if isinstance(target, Minion):
            if target.dead or target.dormant or \
                    target not in target.owner.board:
                return 0
            if target.immune_attacking:
                return 0
            if target.divine_shield:
                shields = target.marks.get("shield_hits", 1)
                if shields > 1:
                    target.marks["shield_hits"] = shields - 1
                else:
                    target.divine_shield = False
                return 0
            target.damage += amount
            if from_minion is not None and from_minion.poisonous:
                target.pending_destroy = True
            if from_minion is not None and from_minion.lifesteal:
                self.heal(from_minion.owner, from_minion.owner, amount)
            if spell and source is not None and source.marks is not None:
                pass
            self.fire("minion_damaged", target.owner, target, amount, source)
            self._quest_progress(
                self.players[self.current], "damage", target, amount)
            self.recompute_auras()
            return amount
        else:
            pl = target
            if pl.immune:
                return 0
            if pl.hp + pl.armor - amount <= 0 and "Ice Block" in pl.secrets:
                self.pop_secret(pl, "Ice Block")
                pl.immune = True
                return 0
            if pl.marks.get("hero_ds"):
                pl.marks["hero_ds"] = False
                return 0
            absorbed = min(pl.armor, amount)
            pl.armor -= absorbed
            pl.hp -= amount - absorbed
            if from_minion is not None and from_minion.lifesteal:
                self.heal(from_minion.owner, from_minion.owner, amount)
            self.fire("hero_damaged", pl, amount)
            self._quest_progress(
                self.players[self.current], "damage", pl, amount)
            if pl.hp <= 0:
                self._die(pl)
            return amount

    def spell_damage(self, p, target, base):
        mult = 2 if p.marks.get("spell_double") else 1
        dealt = self.deal_damage(p, target, (base + p.spell_power) * mult,
                                 spell=True)
        if dealt > 0:
            p.marks["spell_dmg_turn"] = \
                p.marks.get("spell_dmg_turn", 0) + dealt
            self.fire("spell_dealt_damage", p, dealt)
        return dealt

    def heal(self, source_p, target, amount):
        if self.over or amount <= 0:
            return
        if source_p is not None and source_p.marks.get("spell_double"):
            pass
        if isinstance(target, Minion):
            if target.dead:
                return
            healed = min(target.damage, amount)
            target.damage -= healed
        else:
            healed = min(target.max_hp - target.hp, amount)
            target.hp += healed
        if healed > 0:
            self.fire("healed", target, healed)

    def gain_armor(self, p, n):
        p.armor += n

    def destroy(self, target):
        if isinstance(target, Minion):
            if not target.dormant:
                target.pending_destroy = True
        elif isinstance(target, Location):
            if target in target.owner.board:
                target.owner.board.remove(target)
        else:
            self._die(target)

    def _die(self, player):
        self.over = True
        opp = player.opponent
        self.winner = None if opp.hp <= 0 else opp.idx

    # ---------------------------------------------------------------- combat
    def attack(self, p, attacker, target):
        opp = p.opponent
        if isinstance(attacker, Minion):
            if "Freezing Trap" in opp.secrets:
                self.pop_secret(opp, "Freezing Trap")
                if attacker in p.board:
                    p.board.remove(attacker)
                    from .carddata import make_inst
                    inst = make_inst(attacker.card.id)
                    inst.cost_delta = 2
                    if len(p.hand) < MAX_HAND:
                        p.hand.append(inst)
                    self.recompute_auras()
                return
        if "Explosive Trap" in opp.secrets and target is opp:
            self.pop_secret(opp, "Explosive Trap")
            for m in list(p.minions):
                self.deal_damage(opp, m, 2)
            self.deal_damage(opp, p, 2)
            self.check_deaths()
            if self.over or (isinstance(attacker, Minion) and attacker.dead):
                return
        if "Ice Barrier" in opp.secrets and target is opp:
            self.pop_secret(opp, "Ice Barrier")
            opp.armor += 8

        if isinstance(attacker, Minion):
            if attacker.dead or attacker not in p.board:
                return
            attacker.attacks_done += 1
            attacker.stealth = False
            atk_val = attacker.attack
            if isinstance(target, Minion):
                if target.dead or target not in target.owner.board:
                    return
                ret = target.attack
                self.deal_damage(p, target, atk_val, from_minion=attacker)
                self.deal_damage(opp, attacker, ret, from_minion=target)
            else:
                self.deal_damage(p, target, atk_val, from_minion=attacker)
        else:  # hero attack
            p.hero_attacks += 1
            p.hero_attacked_turn = True
            p.hero_attacks_game += 1
            atk_val = p.hero_attack
            killed = None
            if p.weapon:
                fn = p.weapon.triggers.get("hero_attack")
                if fn:
                    fn(self, p)
                if p.weapon.lifesteal:
                    self.heal(None, p, atk_val)
            if isinstance(target, Minion):
                ret = target.attack
                self.deal_damage(p, target, atk_val)
                if target.dead:
                    killed = target
                self.deal_damage(opp, p, ret)
            else:
                self.deal_damage(p, target, atk_val)
            if p.weapon:
                p.weapon.dur -= 1
                if p.weapon.dur <= 0:
                    self._break_weapon(p)
            self.fire("hero_attacked", p, killed)
        self.check_deaths()

    def freeze(self, target):
        if isinstance(target, Minion):
            own_turn = self.players[self.current] is target.owner
            if own_turn and target.attacks_done == 0 \
                    and not target.just_summoned:
                target.frozen = 1
            elif own_turn:
                target.frozen = 2
            else:
                target.frozen = 1
        elif isinstance(target, Player):
            p = target
            own_turn = self.players[self.current] is p
            p.hero_frozen = 2 if (own_turn and p.hero_attacks > 0) else 1

    def _break_weapon(self, p):
        w = p.weapon
        p.weapon = None
        if w and w.deathrattle:
            w.deathrattle(self, p, w)
            self.check_deaths()

    def destroy_weapon(self, p):
        self._break_weapon(p)

    def pop_secret(self, p, name):
        p.secrets.remove(name)
        self.fire("secret_revealed", p, name)

    # ---------------------------------------------------------------- deaths
    def check_deaths(self):
        if self.over:
            return
        for _ in range(30):
            dead = [m for pl in self.players for m in pl.minions if m.dead]
            if not dead:
                break
            for m in dead:
                if m in m.owner.board:
                    m.owner.board.remove(m)
            self.recompute_auras()
            for m in dead:
                own = m.owner
                own.minions_died_turn += 1
                own.minions_died_game += 1
                gain = 2 if any(x.card.name == "Falric" and not x.silenced
                                for x in own.active_minions) else 1
                own.corpses += gain
                if m.deathrattles:
                    own.graveyard_dr.append(m.card)
                for dr in m.deathrattles:
                    dr(self, own, m)
                if m.reborn:
                    nm = self.summon(own, m.card)
                    if nm is not None:
                        nm.reborn = False
                        nm.damage = nm.max_hp - 1
                self.fire("minion_died", own, m)
                self._quest_progress(own, "minion_died", m)
                if self.over:
                    return
            self.recompute_auras()

    # ----------------------------------------------------------------- auras
    def recompute_auras(self):
        for p in self.players:
            for m in p.minions:
                m.aura_atk = 0
                m.aura_hp = 0
        for p in self.players:
            for m in p.minions:
                if m.aura and not m.silenced and not m.dormant:
                    m.aura(self, p, m)

    def adjacent(self, m):
        board = m.owner.board
        if m not in board:
            return []
        i = board.index(m)
        out = []
        if i > 0 and isinstance(board[i - 1], Minion):
            out.append(board[i - 1])
        if i + 1 < len(board) and isinstance(board[i + 1], Minion):
            out.append(board[i + 1])
        return out

    # --------------------------------------------------------------- events
    def fire(self, event, *args):
        for p in self.players:
            for m in list(p.board):
                if isinstance(m, Minion):
                    fn = m.triggers.get(event)
                    if fn and not m.silenced and not m.dead and not m.dormant:
                        fn(self, p, m, *args)
                        if self.over:
                            return
            if p.weapon:
                fn = p.weapon.triggers.get(event)
                if fn and event != "hero_attack":
                    fn(self, p, *args)
                    if self.over:
                        return
            for entry in list(p.listeners):
                if entry.event != event:
                    continue
                fn = HANDLERS.get(entry.handler_id)
                if fn is None:
                    continue
                fn(self, p, self.by_eid(entry.source_eid), *args,
                   **entry.args)
                if self.over:
                    return
        if event == "minion_played":
            owner, minion = args[0], args[1]
            opp = owner.opponent
            if "Mirror Entity" in opp.secrets:
                self.pop_secret(opp, "Mirror Entity")
                if len(opp.board) < MAX_BOARD and not minion.dead:
                    self.summon(opp, minion.card)

    # ------------------------------------------------------------- discover
    def discover(self, p, options, ctx=None, pick=None):
        """options: list of CardDef or CardInst. Returns the pick or None.

        `pick` (or the head of `self._forced_picks`) short-circuits the RNG
        so a replayed decision does not re-roll.  **Every** impls/autogen
        discover must come through here or `forced_picks` is not honoured.
        """
        options = [o for o in options if o is not None]
        if not options:
            self._last_discover = ([], None)
            return None
        # Sample first either way, so a forced replay burns exactly the
        # same RNG as the original line did.
        picks = self.rng.sample(options, min(3, len(options)))
        if pick is None and self._forced_picks:
            pick = self._forced_picks.pop(0)
        chosen = resolve_pick(pick, options) if pick is not None else None
        if chosen is not None and chosen not in picks:
            picks = [chosen] + picks[1:]
        if chosen is None:
            chosen = (picks[0] if self.agents is None else
                      self.agents[p.idx].choose_discover(self, p, picks,
                                                         ctx))
        self._last_discover = (list(picks), chosen)
        return chosen

    # ----------------------------------------------------------- hero power
    def use_hero_power(self, p, target=None, which=None):
        hp = which or p.hero_power
        if hp is None or hp.passive:
            return False
        if hp.used >= hp.uses_per_turn:
            return False
        if hp.corpse_cost:
            if p.corpses < hp.corpse_cost:
                return False
        if p.mana < hp.cost:
            return False
        p.mana -= hp.cost
        p.marks["mana_spent"] = p.marks.get("mana_spent", 0) + hp.cost
        if hp.corpse_cost:
            p.corpses -= hp.corpse_cost
        hp.used += 1
        if hp.use_fn:
            hp.use_fn(self, p, target)
        self.fire("hero_power_used", p)
        self.check_deaths()
        return True

    # ----------------------------------------------------------- cloning
    def clone(self):
        """Copy deep enough for search. `copy.deepcopy`/`pickle` rejected.

        `_by_eid` is the copy source of truth — zone lists are only
        *indexes* and get rewritten by eid afterwards, so off-board
        instances (Irida's `void`, Godfrey's overflow) survive the copy
        instead of being silently shared with the original.
        """
        self._ensure_eids()
        g = Game.__new__(type(self))
        g.rng = random.Random()
        g.rng.setstate(self.rng.getstate())
        g.agents = self.agents
        g.turn = self.turn
        g.current = self.current
        g.over = self.over
        g.winner = self.winner
        g._outcast = False          # ephemeral: reset, never carried over
        g._current_inst = None      # ephemeral: reset
        g._forced_picks = None
        g._last_discover = None
        g._next_eid = self._next_eid
        g._by_eid = {}
        g.players = []
        # 1. players keep eids 1|2
        for p in self.players:
            q = Player.__new__(Player)
            for slot in Player.__slots__:
                setattr(q, slot, getattr(p, slot, None))
            q.game = g
            g.players.append(q)
            g._by_eid[q.eid] = q
        # 2. every non-player entity, copied straight off _by_eid
        for eid, obj in self._by_eid.items():
            if isinstance(obj, Player):
                continue
            g._by_eid[eid] = _copy_entity(obj)
        # 3. re-link owners; remap any entity refs left inside marks
        for obj in g._by_eid.values():
            if isinstance(obj, Player):
                continue
            own = getattr(obj, "owner", None)
            if isinstance(own, Player):
                obj.owner = g.players[own.idx]
            if getattr(obj, "marks", None):
                obj.marks = _remap(obj.marks, g)
        # 4. zone lists rebuilt by eid — never copied by reference
        for p, q in zip(self.players, g.players):
            q.hand = [g._by_eid[i.eid] for i in p.hand]
            q.deck = [g._by_eid[i.eid] for i in p.deck]
            q.board = [g._by_eid[i.eid] for i in p.board]
            q.void = [g._by_eid[i.eid] for i in p.void]
            q.secrets = list(p.secrets)
            q.weapon = g._by_eid[p.weapon.eid] if p.weapon else None
            q.hero_power = (g._by_eid[p.hero_power.eid]
                            if p.hero_power else None)
            q.hero_power2 = (g._by_eid[p.hero_power2.eid]
                             if p.hero_power2 else None)
            # 5. pending effects are dataclasses: copy, never share
            q.listeners = [l.copy() for l in p.listeners]
            q.turn_start_fx = [f.copy() for f in p.turn_start_fx]
            q.played_types_turn = set(p.played_types_turn)
            q.played_types_last = set(p.played_types_last)
            q.spell_schools_turn = set(p.spell_schools_turn)
            q.spell_schools_last = set(p.spell_schools_last)
            q.played_cards_turn = list(p.played_cards_turn)
            q.dead_minion_cards = list(p.dead_minion_cards)
            q.graveyard_dr = list(p.graveyard_dr)
            # quest callables live on the CardDef, so a shallow dict is safe
            q.quest = dict(p.quest) if p.quest else None
            q.sidequest = dict(p.sidequest) if p.sidequest else None
            q.marks = _remap(p.marks, g)
        return g


ENTITY_TYPES = (CardInst, Minion, Weapon, Location, HeroPowerState)


def _copy_entity(o):
    """Slot-copy one entity; containers copied, CardDefs/functions shared."""
    cls = type(o)
    n = cls.__new__(cls)
    for slot in cls.__slots__:
        v = getattr(o, slot, None)
        if type(v) is dict:
            v = dict(v)
        elif type(v) is list:
            v = list(v)
        elif type(v) is set:
            v = set(v)
        setattr(n, slot, v)
    return n


def _remap(v, g):
    """Rewrite entity references onto the clone's entity universe."""
    if isinstance(v, Player):
        return g.players[v.idx]
    if isinstance(v, ENTITY_TYPES):
        return g._by_eid.get(v.eid, v)
    if type(v) is list:
        return [_remap(x, g) for x in v]
    if type(v) is tuple:
        return tuple(_remap(x, g) for x in v)
    if type(v) is dict:
        return {k: _remap(x, g) for k, x in v.items()}
    if type(v) is set:
        return set(v)
    return v


def _sweep_marks(game, v):
    """Register any entity hiding inside a marks value."""
    if isinstance(v, ENTITY_TYPES):
        game.reg(v)
    elif type(v) in (list, tuple, set):
        for x in v:
            _sweep_marks(game, x)
    elif type(v) is dict:
        for x in v.values():
            _sweep_marks(game, x)


def resolve_pick(want, options):
    """Map one forced pick onto `options` (CardDefs or CardInsts).

    Accepts the option object itself, an eid, an index, a card id, or a
    card name.  Returns None when nothing matches, so the caller falls
    back to the agent and knows the line is no longer forced.
    """
    if want is None:
        return None
    for o in options:
        if o is want:
            return o
    if isinstance(want, bool):
        return None
    if isinstance(want, int):
        for o in options:
            if getattr(o, "eid", 0) == want:
                return o
        return options[want] if 0 <= want < len(options) else None
    if isinstance(want, str):
        for o in options:
            card = getattr(o, "card", o)
            if want in (getattr(card, "id", None),
                        getattr(card, "name", None)):
                return o
    return None
