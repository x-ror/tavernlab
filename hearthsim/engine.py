"""Core Hearthstone game engine.

Implements the rules needed for correct simulation of the classic card pool:
mana/overload, fatigue, board limit 7, hand limit 10 (burn), summoning
sickness, charge/rush/windfury, taunt, divine shield, stealth, poisonous,
lifesteal, freeze, silence, spell damage, secrets, weapons, auras
(positional and global), enrage, battlecries, deathrattles, triggers,
combo, choose-one, outcast, temporary buffs, simultaneous combat damage.
"""
import random

MINION, SPELL, WEAPON = "M", "S", "W"
MAX_BOARD = 7
MAX_HAND = 10
MAX_MANA = 10
TURN_LIMIT = 89  # official rule: game is a draw at turn 89


class CardDef:
    __slots__ = (
        "name", "cls", "cost", "type", "atk", "hp", "dur", "tribe",
        "taunt", "divine_shield", "charge", "rush", "windfury", "stealth",
        "lifesteal", "poisonous", "spell_dmg", "cant_attack",
        "battlecry", "deathrattle", "triggers", "aura", "spell",
        "target", "battlecry_target", "combo", "combo_spell", "overload",
        "secret", "choose", "outcast_discount", "enrage_atk",
        "ai_hint", "play_if", "token", "collectible",
    )

    def __init__(self, name, cls, cost, type_, atk=0, hp=0, dur=0, tribe=None,
                 taunt=False, divine_shield=False, charge=False, rush=False,
                 windfury=False, stealth=False, lifesteal=False,
                 poisonous=False, spell_dmg=0, cant_attack=False,
                 battlecry=None, deathrattle=None, triggers=None, aura=None,
                 spell=None, target=None, battlecry_target=None,
                 combo=None, combo_spell=None, overload=0, secret=None,
                 choose=None, outcast_discount=0, enrage_atk=0,
                 ai_hint=None, play_if=None, token=False):
        self.name = name
        self.cls = cls
        self.cost = cost
        self.type = type_
        self.atk = atk
        self.hp = hp
        self.dur = dur
        self.tribe = tribe
        self.taunt = taunt
        self.divine_shield = divine_shield
        self.charge = charge
        self.rush = rush
        self.windfury = windfury
        self.stealth = stealth
        self.lifesteal = lifesteal
        self.poisonous = poisonous
        self.spell_dmg = spell_dmg
        self.cant_attack = cant_attack
        self.battlecry = battlecry
        self.deathrattle = deathrattle
        self.triggers = triggers or {}
        self.aura = aura
        self.spell = spell
        self.target = target                  # spell target requirement
        self.battlecry_target = battlecry_target
        self.combo = combo                    # combo battlecry (rogue)
        self.combo_spell = combo_spell        # combo spell variant
        self.overload = overload
        self.secret = secret                  # secret id string
        self.choose = choose                  # (fn_a, fn_b) choose-one
        self.outcast_discount = outcast_discount
        self.enrage_atk = enrage_atk
        self.ai_hint = ai_hint or ()
        self.play_if = play_if
        self.token = token


class Minion:
    __slots__ = (
        "card", "owner", "atk_base", "hp_base", "perm_atk", "perm_hp",
        "temp_atk", "aura_atk", "aura_hp", "damage",
        "taunt", "divine_shield", "charge", "rush", "windfury", "stealth",
        "lifesteal", "poisonous", "cant_attack", "spell_dmg",
        "frozen", "silenced", "attacks_done", "just_summoned",
        "deathrattles", "triggers", "aura", "enrage_atk", "pending_destroy",
        "to_die_eot", "tribe",
    )

    def __init__(self, card, owner):
        self.card = card
        self.owner = owner
        self.atk_base = card.atk
        self.hp_base = card.hp
        self.perm_atk = 0
        self.perm_hp = 0
        self.temp_atk = 0
        self.aura_atk = 0
        self.aura_hp = 0
        self.damage = 0
        self.taunt = card.taunt
        self.divine_shield = card.divine_shield
        self.charge = card.charge
        self.rush = card.rush
        self.windfury = card.windfury
        self.stealth = card.stealth
        self.lifesteal = card.lifesteal
        self.poisonous = card.poisonous
        self.cant_attack = card.cant_attack
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
        self.tribe = card.tribe

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
        return self.health <= 0 or self.pending_destroy

    def can_attack(self):
        if self.cant_attack or self.frozen or self.attack <= 0:
            return False
        max_attacks = 2 if self.windfury else 1
        if self.attacks_done >= max_attacks:
            return False
        if self.just_summoned and not (self.charge or self.rush):
            return False
        return True

    def can_attack_face(self):
        return self.can_attack() and not (self.just_summoned and self.rush
                                          and not self.charge)

    def silence(self):
        self.silenced = True
        self.taunt = self.divine_shield = self.windfury = False
        self.stealth = self.lifesteal = self.poisonous = False
        self.cant_attack = False
        self.spell_dmg = 0
        self.perm_atk = min(self.perm_atk, 0)
        self.perm_hp = min(self.perm_hp, 0)
        self.temp_atk = 0
        self.frozen = 0
        self.deathrattles = []
        self.triggers = {}
        self.aura = None
        self.enrage_atk = 0

    def heal_full(self):
        self.damage = 0


class Weapon:
    __slots__ = ("card", "atk", "dur", "windfury", "lifesteal", "deathrattle")

    def __init__(self, card):
        self.card = card
        self.atk = card.atk
        self.dur = card.dur
        self.windfury = card.windfury
        self.lifesteal = card.lifesteal
        self.deathrattle = card.deathrattle


class Player:
    __slots__ = (
        "game", "idx", "cls", "hp", "armor", "temp_atk", "weapon",
        "hand", "deck", "board", "secrets", "mana", "crystals",
        "overload_next", "overload_now", "fatigue", "hp_used",
        "hero_attacks", "hero_frozen", "cards_played_turn",
        "spell_discount", "archetype", "immune", "hero_attacked_turn",
    )

    def __init__(self, game, idx, cls, deck_cards, archetype):
        self.game = game
        self.idx = idx
        self.cls = cls
        self.hp = 30
        self.armor = 0
        self.temp_atk = 0
        self.weapon = None
        self.hand = []
        self.deck = list(deck_cards)
        self.board = []
        self.secrets = []
        self.mana = 0
        self.crystals = 0
        self.overload_next = 0
        self.overload_now = 0
        self.fatigue = 0
        self.hp_used = False
        self.hero_attacks = 0
        self.hero_frozen = 0
        self.cards_played_turn = 0
        self.spell_discount = 0
        self.archetype = archetype
        self.immune = False
        self.hero_attacked_turn = False

    @property
    def opponent(self):
        return self.game.players[1 - self.idx]

    @property
    def hero_attack(self):
        atk = self.temp_atk
        if self.weapon:
            atk += self.weapon.atk
        return atk

    @property
    def spell_power(self):
        return sum(m.spell_dmg for m in self.board)

    def hero_can_attack(self):
        if self.hero_frozen or self.hero_attack <= 0:
            return False
        max_att = 2 if (self.weapon and self.weapon.windfury) else 1
        return self.hero_attacks < max_att

    def effective_cost(self, card):
        cost = card.cost
        if card.type == SPELL:
            cost -= self.spell_discount
            for m in self.board:
                if m.card.name == "Sorcerer's Apprentice":
                    cost -= 1
        if card.name == "Mountain Giant":
            cost -= (len(self.hand) - 1)
        if card.name == "Sea Giant":
            cost -= len(self.board) + len(self.opponent.board)
        if card.outcast_discount and self.hand and \
                (self.hand[0] is card or self.hand[-1] is card):
            cost -= card.outcast_discount
        return max(0, cost)

    def is_outcast(self, card):
        return self.hand and (self.hand[0] is card or self.hand[-1] is card)


class Game:
    def __init__(self, deck0, deck1, seed=None, agents=None):
        """deck0/deck1: objects with .cls, .cards (list of CardDef), .archetype"""
        self.rng = random.Random(seed)
        self.players = [
            Player(self, 0, deck0.cls, deck0.cards, deck0.archetype),
            Player(self, 1, deck1.cls, deck1.cards, deck1.archetype),
        ]
        self.turn = 0
        self.current = 0
        self.over = False
        self.winner = None
        self.agents = agents
        self._outcast = False

    def pop_secret(self, p, name):
        """Remove a revealed secret; Eaglehorn Bow gains durability."""
        p.secrets.remove(name)
        if p.weapon and p.weapon.card.name == "Eaglehorn Bow":
            p.weapon.dur += 1

    # ------------------------------------------------------------------ setup
    def start(self, first=0):
        from .cards import get_card
        self.current = first
        for p in self.players:
            self.rng.shuffle(p.deck)
        first_p, second_p = self.players[first], self.players[1 - first]
        self._mulligan(first_p, 3)
        self._mulligan(second_p, 4)
        second_p.hand.append(get_card("The Coin"))

    def _mulligan(self, p, n):
        # keep cards costing <= threshold (aggro 2, else 3); redraw the rest
        thresh = 2 if p.archetype == "aggro" else 3
        drawn = [p.deck.pop() for _ in range(n)]
        keep = [c for c in drawn if c.cost <= thresh]
        back = [c for c in drawn if c.cost > thresh]
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
            if self.over:
                break
            self.agents[p.idx].take_turn(self, p)
            if self.over:
                break
            self.end_turn(p)
            self.current = 1 - self.current
        return self.winner

    def begin_turn(self, p):
        if p.crystals < MAX_MANA:
            p.crystals += 1
        p.overload_now = p.overload_next
        p.overload_next = 0
        p.mana = max(0, p.crystals - p.overload_now)
        p.hp_used = False
        p.hero_attacks = 0
        p.hero_attacked_turn = False
        p.cards_played_turn = 0
        p.temp_atk = 0
        for m in p.board:
            m.attacks_done = 0
            m.just_summoned = False
        self.draw(p)
        self.fire("turn_start", p)
        self.check_deaths()

    def end_turn(self, p):
        self.fire("turn_end", p)
        self.check_deaths()
        # temporary effects expire
        p.temp_atk = 0
        p.spell_discount = 0
        for m in list(p.board):
            m.temp_atk = 0
            if m.to_die_eot:
                m.pending_destroy = True
        # thaw
        for m in p.board:
            if m.frozen:
                m.frozen -= 1
        if p.hero_frozen:
            p.hero_frozen -= 1
        self.check_deaths()
        self.recompute_auras()

    # ------------------------------------------------------------------ draws
    def draw(self, p, n=1):
        from .cards import get_card
        for _ in range(n):
            if not p.deck:
                p.fatigue += 1
                self.deal_damage(None, p, p.fatigue, ignore_spellpower=True)
                if self.over:
                    return None
                continue
            card = p.deck.pop()
            if len(p.hand) >= MAX_HAND:
                continue  # card is burned
            p.hand.append(card)
            if p.cls == "Priest":
                pass
        return None

    def add_to_hand(self, p, card):
        if len(p.hand) < MAX_HAND:
            p.hand.append(card)

    # ---------------------------------------------------------------- summons
    def summon(self, p, card, position=None):
        if len(p.board) >= MAX_BOARD:
            return None
        m = Minion(card, p)
        if position is None:
            p.board.append(m)
        else:
            p.board.insert(position, m)
        self.recompute_auras()
        self.fire("summon", p, m)
        return m

    def play_card(self, p, card, target=None, choice=None, position=None):
        cost = p.effective_cost(card)
        if cost > p.mana:
            return False
        outcast = p.is_outcast(card) if card.outcast_discount else False
        combo_active = p.cards_played_turn > 0
        p.mana -= cost
        if card.type == SPELL and p.spell_discount:
            p.spell_discount = 0
        p.hand.remove(card)
        p.cards_played_turn += 1
        if card.overload:
            p.overload_next += card.overload
            self.fire("overload", p)

        if card.type == MINION:
            if len(p.board) >= MAX_BOARD:
                return True  # rules: can't be played; guard in AI. mana safety
            m = Minion(card, p)
            if position is None:
                p.board.append(m)
            else:
                p.board.insert(position, m)
            self.recompute_auras()
            bc = card.combo if (combo_active and card.combo) else card.battlecry
            if card.choose:
                bc = card.choose[choice or 0]
                bc(self, p, m, target)
                bc = None
            if bc:
                bc(self, p, m, target)
            if not self.over:
                self.fire("minion_played", p, m)
            self.check_deaths()
        elif card.type == SPELL:
            # secrets: Counterspell
            opp = p.opponent
            if any(s == "Counterspell" for s in opp.secrets):
                self.pop_secret(opp, "Counterspell")
                return True
            if card.secret:
                if card.secret not in p.secrets and len(p.secrets) < 5:
                    p.secrets.append(card.secret)
            else:
                fn = card.spell
                if combo_active and card.combo_spell:
                    fn = card.combo_spell
                self._outcast = outcast
                if card.choose:
                    fn = card.choose[choice or 0]
                if target is not None and self._gone(target):
                    target = None
                fn(self, p, target)
            self.fire("spell_cast", p, card)
            self.check_deaths()
        elif card.type == WEAPON:
            p.weapon = Weapon(card)
            if card.battlecry:
                card.battlecry(self, p, None, target)
            self.check_deaths()

        if card.type == MINION and outcast:
            pass
        if outcast and card.name == "Skull of Gul'dan":
            pass  # handled in spell fn via outcast flag - see cards.py
        return True

    def _gone(self, target):
        if isinstance(target, Minion):
            return target.dead or target not in target.owner.board
        return False

    # ---------------------------------------------------------------- damage
    def deal_damage(self, source, target, amount, ignore_spellpower=True,
                    from_minion=None):
        """source: Player causing it (or None). target: Minion or Player."""
        if amount <= 0 or self.over:
            return 0
        if isinstance(target, Minion):
            if target.dead or target not in target.owner.board:
                return 0
            if target.divine_shield:
                target.divine_shield = False
                return 0
            target.damage += amount
            if from_minion is not None and from_minion.poisonous:
                target.pending_destroy = True
            if from_minion is not None and from_minion.lifesteal:
                self.heal(from_minion.owner, from_minion.owner, amount)
            self.fire("minion_damaged", target.owner, target, amount, source)
            self.recompute_auras()
            return amount
        else:  # hero
            pl = target
            if pl.immune:
                return 0
            # Ice Block
            total_after = pl.hp + pl.armor - amount
            if total_after <= 0 and "Ice Block" in pl.secrets:
                self.pop_secret(pl, "Ice Block")
                pl.immune = True  # immune for the rest of this turn
                return 0
            absorbed = min(pl.armor, amount)
            pl.armor -= absorbed
            rest = amount - absorbed
            pl.hp -= rest
            if from_minion is not None and from_minion.lifesteal:
                self.heal(from_minion.owner, from_minion.owner, amount)
            self.fire("hero_damaged", pl, amount)
            if pl.hp <= 0:
                self._die(pl)
            return amount

    def spell_damage(self, p, target, base):
        """Damage from a spell: applies spell power."""
        return self.deal_damage(p, target, base + p.spell_power)

    def heal(self, source_p, target, amount):
        if self.over:
            return
        # Auchenai Soulpriest: friendly healing becomes damage
        if source_p is not None and any(
                m.card.name == "Auchenai Soulpriest" and not m.silenced
                for m in source_p.board):
            self.deal_damage(source_p, target, amount)
            return
        if isinstance(target, Minion):
            if target.dead:
                return
            healed = min(target.damage, amount)
            target.damage -= healed
        else:
            healed = min(30 - target.hp, amount)
            target.hp += healed
        if healed > 0:
            self.fire("healed", target, healed)

    def gain_armor(self, p, n):
        p.armor += n

    def destroy(self, target):
        if isinstance(target, Minion):
            target.pending_destroy = True
        else:
            self._die(target)

    def _die(self, player):
        self.over = True
        opp = player.opponent
        if opp.hp <= 0:
            self.winner = None  # simultaneous death: draw
        else:
            self.winner = opp.idx

    # ---------------------------------------------------------------- combat
    def attack(self, p, attacker, target):
        """attacker: Minion or 'hero'; target: Minion or opponent Player."""
        opp = p.opponent
        # secrets fire before combat
        if isinstance(attacker, Minion):
            if "Freezing Trap" in opp.secrets:
                self.pop_secret(opp, "Freezing Trap")
                if attacker in p.board:
                    p.board.remove(attacker)
                    from .cards import get_card
                    if len(p.hand) < MAX_HAND:
                        p.hand.append(attacker.card)
                    self.recompute_auras()
                return
            if "Vaporize" in opp.secrets and target is opp:
                self.pop_secret(opp, "Vaporize")
                self.destroy(attacker)
                self.check_deaths()
                return
        if "Explosive Trap" in opp.secrets and target is opp:
            self.pop_secret(opp, "Explosive Trap")
            for m in list(p.board):
                self.deal_damage(opp, m, 2)
            self.deal_damage(opp, p, 2)
            self.check_deaths()
            if self.over:
                return
            if isinstance(attacker, Minion) and attacker.dead:
                return
        if "Noble Sacrifice" in opp.secrets:
            self.pop_secret(opp, "Noble Sacrifice")
            from .cards import get_card
            defender = self.summon(opp, get_card("Defender"))
            if defender is not None:
                target = defender
        if "Ice Barrier" in opp.secrets and target is opp:
            self.pop_secret(opp, "Ice Barrier")
            opp.armor += 8
        if isinstance(target, Minion) and "Snake Trap" in opp.secrets \
                and target.owner is opp:
            self.pop_secret(opp, "Snake Trap")
            from .cards import get_card
            for _ in range(3):
                self.summon(opp, get_card("Snake"))

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
                self._combat_triggers(attacker, target)
            else:
                self.deal_damage(p, target, atk_val, from_minion=attacker)
                self._combat_triggers(attacker, None)
        else:  # hero attack
            p.hero_attacks += 1
            p.hero_attacked_turn = True
            atk_val = p.hero_attack
            if p.weapon:
                fn = p.weapon.card.triggers.get("hero_attack")
                if fn:
                    fn(self, p)
                if p.weapon.lifesteal:
                    self.heal(None, p, atk_val)
            if isinstance(target, Minion):
                ret = target.attack
                self.deal_damage(p, target, atk_val)
                if target.poisonous:
                    pass  # poisonous does not destroy heroes
                self.deal_damage(opp, p, ret)
            else:
                self.deal_damage(p, target, atk_val)
            if p.weapon:
                p.weapon.dur -= 1
                if p.weapon.dur <= 0:
                    self._break_weapon(p)
            self.fire("hero_attacked", p)
        self.check_deaths()

    def _combat_triggers(self, attacker, target):
        # Water Elemental style freeze
        if attacker.card.name == "Water Elemental" and target is not None:
            self._freeze_minion(target)
        if target is not None and target.card.name == "Water Elemental":
            self._freeze_minion(attacker)

    def freeze(self, target):
        if isinstance(target, Minion):
            self._freeze_minion(target)
        else:
            p = target
            own_turn = self.players[self.current] is p
            p.hero_frozen = 2 if (own_turn and p.hero_attacks > 0) else 1
            if own_turn and p.hero_attacks == 0:
                p.hero_frozen = 1

    def _freeze_minion(self, m):
        own_turn = self.players[self.current] is m.owner
        if own_turn and m.attacks_done == 0 and not m.just_summoned:
            m.frozen = 1
        elif own_turn:
            m.frozen = 2
        else:
            m.frozen = 1

    def _break_weapon(self, p):
        w = p.weapon
        p.weapon = None
        if w and w.deathrattle:
            w.deathrattle(self, p, None)
            self.check_deaths()

    # ---------------------------------------------------------------- deaths
    def check_deaths(self):
        if self.over:
            return
        for _ in range(30):
            dead = []
            for p in self.players:
                for m in p.board:
                    if m.dead:
                        dead.append(m)
            if not dead:
                break
            for m in dead:
                if m in m.owner.board:
                    m.owner.board.remove(m)
            self.recompute_auras()
            for m in dead:
                for dr in m.deathrattles:
                    dr(self, m.owner, m)
                self.fire("minion_died", m.owner, m)
                if self.over:
                    return
            self.recompute_auras()

    # ----------------------------------------------------------------- auras
    def recompute_auras(self):
        for p in self.players:
            for m in p.board:
                m.aura_atk = 0
                m.aura_hp = 0
        for p in self.players:
            for m in p.board:
                if m.aura and not m.silenced:
                    m.aura(self, p, m)

    def adjacent(self, m):
        board = m.owner.board
        if m not in board:
            return []
        i = board.index(m)
        out = []
        if i > 0:
            out.append(board[i - 1])
        if i + 1 < len(board):
            out.append(board[i + 1])
        return out

    # --------------------------------------------------------------- events
    def fire(self, event, *args):
        for p in self.players:
            for m in list(p.board):
                fn = m.triggers.get(event)
                if fn and not m.silenced and not m.dead:
                    fn(self, p, m, *args)
                    if self.over:
                        return
        # Mirror Entity is handled here (opponent plays a minion)
        if event == "minion_played":
            owner, minion = args[0], args[1]
            opp = owner.opponent
            if "Mirror Entity" in opp.secrets:
                self.pop_secret(opp, "Mirror Entity")
                if len(opp.board) < MAX_BOARD and not minion.dead:
                    self.summon(opp, minion.card)

    # ----------------------------------------------------------- hero power
    def use_hero_power(self, p, target=None):
        from .cards import get_card, TOTEMS
        if p.hp_used or p.mana < 2:
            return False
        cost = 2
        if p.cls == "Demon Hunter":
            cost = 1
        if p.mana < cost:
            return False
        p.mana -= cost
        p.hp_used = True
        opp = p.opponent
        c = p.cls
        if c == "Mage":
            self.deal_damage(p, target if target is not None else opp, 1)
        elif c == "Hunter":
            self.deal_damage(p, opp, 2)
        elif c == "Warrior":
            self.gain_armor(p, 2)
        elif c == "Warlock":
            self.deal_damage(None, p, 2)
            if not self.over:
                self.draw(p)
        elif c == "Priest":
            tgt = target if target is not None else p
            self.heal(p, tgt, 2)
        elif c == "Paladin":
            self.summon(p, get_card("Silver Hand Recruit"))
        elif c == "Shaman":
            have = {m.card.name for m in p.board}
            options = [t for t in TOTEMS if t not in have]
            if options:
                self.summon(p, get_card(self.rng.choice(options)))
        elif c == "Druid":
            p.temp_atk += 1
            self.gain_armor(p, 1)
        elif c == "Rogue":
            p.weapon = Weapon(get_card("Wicked Knife"))
        elif c == "Demon Hunter":
            p.temp_atk += 1
        self.check_deaths()
        return True
