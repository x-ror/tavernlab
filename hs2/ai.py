"""Heuristic agent for engine v2."""
from .engine import Minion, Location, MINION, SPELL, WEAPON, LOCATION, \
    HERO, MAX_BOARD

BURN = {"Fireball": 6, "Frostbolt": 3, "Arcane Barrage": 3,
        "Arcane Flow": 6, "Sleet Storm": 2, "Press the Advantage": 1,
        "Moonwell": 4, "Cosmic Manifestations": 2, "Ebb and Flow": 3,
        "Rite of Twilight": 3, "Wound Prey": 1}


def minion_value(m):
    v = m.attack + m.health
    if m.taunt:
        v += 1
    if m.divine_shield:
        v += 2
    if m.deathrattles:
        v += 1
    if m.aura or m.triggers:
        v += 2
    if m.spell_dmg:
        v += 1
    return v


def biggest(minions):
    live = [m for m in minions if not m.dead]
    return max(live, key=minion_value) if live else None


class Agent:
    def __init__(self, style="midrange"):
        self.style = style

    # ------------------------------------------------------------- choices
    def choose_discover(self, game, p, picks, ctx=None):
        """`picks` may be CardDefs or CardInsts (Game.discover accepts
        both, so log-replay can force a pick by entity id)."""
        def score(o):
            c = getattr(o, "card", o)
            s = c.cost
            if c.type == MINION:
                s += (c.atk + c.hp) * 0.3
            if c.cost <= p.crystals + 1:
                s += 2
            return s
        return max(picks, key=score)

    def choose_cataclysms(self, game, p, options, n):
        opp = p.opponent
        scores = {}
        enemy_board_hp = sum(m.health for m in opp.active_minions)
        big = biggest(opp.active_minions)
        scores["Raze"] = 2 + enemy_board_hp * 0.5
        scores["Topple"] = 3 + (minion_value(big) if big else 0)
        scores["Dragon's Reign"] = 8 if len(p.board) < MAX_BOARD else 0
        scores["Enthrall"] = 4 if len(p.deck) > 5 else 1
        ranked = sorted(options, key=lambda o: -scores.get(o, 0))
        return ranked[:n]

    # ------------------------------------------------------------ main loop
    def take_turn(self, game, p):
        for _ in range(100):
            if game.over:
                return
            if self.try_lethal(game, p):
                return
            if self.use_location(game, p):
                continue
            if self.play_best_card(game, p):
                continue
            if self.use_hero_power(game, p):
                continue
            if self.do_attack(game, p):
                continue
            if self.try_prepare(game, p):
                continue
            break

    # --------------------------------------------------------------- lethal
    def face_burn(self, p):
        mana = p.mana
        total = 0
        sp = p.spell_power
        cards = []
        options = []
        for inst in p.hand:
            c = inst.card
            if c.name in BURN and c.target in ("any", "enemy", None) \
                    and c.type == SPELL and inst.locked_turn != p.game.turn \
                    if hasattr(p, 'game') else True:
                pass
        for inst in p.hand:
            c = inst.card
            if c.name in BURN and c.target in ("any", "enemy"):
                options.append((BURN[c.name] + sp,
                                p.effective_cost(inst), inst))
        options.sort(key=lambda x: -x[0] / max(1, x[1]))
        for dmg, cost, inst in options:
            if cost <= mana:
                mana -= cost
                total += dmg
                cards.append(inst)
        return total, cards

    def try_lethal(self, game, p):
        from .lethal import find_lethal, execute
        plan = find_lethal(game, p)
        if plan is None:
            return False
        execute(game, p, plan)
        return True

    # ------------------------------------------------------------ locations
    def use_location(self, game, p):
        for loc in p.locations:
            if not loc.usable():
                continue
            target = None
            if loc.card.name == "Sanguine Depths":
                kill = [m for m in p.opponent.active_minions
                        if m.health == 1]
                own = [m for m in p.active_minions]
                if kill:
                    target = max(kill, key=minion_value)
                elif own:
                    target = max(own, key=lambda m: m.attack)
                else:
                    continue
            if loc.card.name == "Nespirah, Enthralled":
                kill = [m for m in p.opponent.active_minions
                        if m.health == 1]
                target = max(kill, key=minion_value) if kill \
                    else p.opponent
            return game.use_location(p, loc, target)
        return False

    def try_prepare(self, game, p):
        if p.mana <= 0:
            return False
        cands = [i for i in p.hand if i.card.prepare
                 and i.locked_turn != game.turn
                 and p.effective_cost(i) > p.mana + 1]
        if not cands:
            return False
        inst = max(cands, key=lambda i: i.card.cost)
        return game.prepare_card(p, inst)

    # ------------------------------------------------------------ card play
    def play_best_card(self, game, p):
        best = None
        low_deck = len(p.deck) <= 3
        DRAWERS = {"The Unseen Atlas", "Chaos Strike", "Flash of Light",
                   "Press the Advantage", "Tracking"}
        for inst in list(p.hand):
            c = inst.card
            if inst.locked_turn == game.turn:
                continue
            if p.effective_cost(inst) > p.mana:
                continue
            if c.corpse_cost and p.corpses < c.corpse_cost:
                continue
            if c.play_if and not c.play_if(game, p):
                continue
            if low_deck and c.name in DRAWERS and len(p.hand) > 2:
                continue
            res = self.evaluate_play(game, p, inst)
            if res is None:
                continue
            score, target, choice = res
            if score <= 0:
                continue
            if best is None or score > best[0]:
                best = (score, inst, target, choice)
        if best is None:
            return False
        score, inst, target, choice = best
        game.play_card(p, inst, target=target, choice=choice)
        return True

    def evaluate_play(self, game, p, inst):
        c = inst.card
        opp = p.opponent
        name = c.name
        cost = p.effective_cost(inst)
        style = self.style

        if name == "The Coin" or name == "Innervate":
            for other in p.hand:
                if other is inst or other.card.name in ("The Coin",
                                                        "Innervate"):
                    continue
                ec = p.effective_cost(other)
                if p.mana < ec <= p.mana + 1:
                    return (0.5, None, None)
            return None

        # choose-one decisions
        if c.choose:
            return self._choose_play(game, p, inst)

        # AoE style spells with known names
        if name in ("Consecration", "Moonwell", "Hellfire", "Annihilation",
                    "Broxigar's Last Stand", "Shadow Word: Ruin",
                    "Equality", "Judgment", "Lightning Storm"):
            gain = self._aoe_gain(game, p, name)
            return (gain, self._aoe_target(game, p, name),
                    None) if gain > 0 else None

        if c.quest:
            return (9.0, None, None)  # play quests early
        if c.secret:
            return (1.0, None, None)

        if c.type == LOCATION:
            if len(p.board) >= MAX_BOARD:
                return None
            return (cost + 2.0, None, None)

        if c.type == HERO:
            return (cost + 6.0, None, None)

        if c.type == WEAPON:
            if p.weapon and p.weapon.atk * p.weapon.dur >= c.atk * c.dur:
                return None
            return (cost + 1.5, None, None)

        if c.ai_hint:
            res = self._hint_play(game, p, c, cost)
            if res is not None:
                return res

        if c.type == MINION:
            if len(p.board) >= MAX_BOARD:
                return None
            v = cost + 1.0 + (1.0 if (c.charge or c.rush or c.taunt) else 0)
            if c.battlecry_target and c.ai_hint:
                pass
            return (v, None, None)
        if c.type == SPELL:
            if c.target is None:
                return (cost * 0.8 + 0.3, None, None)
            res = self._hint_play(game, p, c, cost)
            return res
        return None

    def _aoe_gain(self, game, p, name):
        opp = p.opponent
        dmg = {"Consecration": 2, "Moonwell": 4, "Hellfire": 3,
               "Broxigar's Last Stand": 1, "Lightning Storm": 3}.get(name)
        if name in ("Shadow Word: Ruin",):
            targets = [m for m in opp.active_minions if m.attack >= 5]
            own = [m for m in p.active_minions if m.attack >= 5]
            return sum(minion_value(m) for m in targets) - \
                sum(minion_value(m) for m in own)
        if name in ("Equality", "Judgment"):
            ov = sum(minion_value(m) for m in opp.active_minions)
            mv = sum(minion_value(m) for m in p.active_minions)
            return 8.0 if ov - mv >= 8 else 0
        if name == "Annihilation":
            ov = sum(minion_value(m) for m in opp.active_minions)
            mv = sum(minion_value(m) for m in p.active_minions)
            return ov - mv
        if dmg is None:
            return 0
        dmg += p.spell_power
        gain = sum(minion_value(m) for m in opp.active_minions
                   if m.health <= dmg)
        gain += sum(1 for m in opp.active_minions if m.health > dmg)
        if name in ("Hellfire", "Broxigar's Last Stand"):
            gain -= sum(minion_value(m) for m in p.active_minions
                        if m.health <= dmg)
        threshold = 4.0 if self.style != "control" else 3.0
        return gain if gain >= threshold else 0

    def _aoe_target(self, game, p, name):
        if name == "Judgment":
            own = p.active_minions
            return max(own, key=lambda m: m.attack + m.health) if own \
                else None
        return None

    def _damage_target(self, game, p, n, allow_face=False,
                       minion_only=False):
        opp = p.opponent
        pool = [m for m in opp.active_minions if not m.stealth]
        kill = [m for m in pool if m.health <= n and not m.dead]
        if kill:
            t = max(kill, key=minion_value)
            return (minion_value(t) + 2 - (n - t.health) * 0.3, t)
        if allow_face and not minion_only:
            if self.style != "control" or opp.hp <= n * 2:
                return (n * 0.45, opp)
        big = [m for m in pool if m.health > n and minion_value(m) >= 8]
        if big and self.style != "aggro":
            return (1.0, max(big, key=minion_value))
        return None

    def _hint_play(self, game, p, c, cost):
        opp = p.opponent
        if not c.ai_hint:
            return None
        kind = c.ai_hint[0]
        base_minion = (cost + 1.0) if c.type == MINION else 0.0

        if kind == "dmg":
            n = c.ai_hint[1] + (p.spell_power if c.type == SPELL else 0)
            res = self._damage_target(
                game, p, n,
                allow_face=(c.target in ("any", "enemy")),
                minion_only=(c.target or "").endswith("minion"))
            if res is None:
                if c.type == MINION and len(p.board) < MAX_BOARD:
                    return (base_minion, None, None)
                return None
            s, t = res
            return (s + base_minion, t, None)
        if kind == "buff":
            targets = [m for m in p.active_minions if not m.dead]
            if not targets:
                return (base_minion * 0.8, None, None) \
                    if c.type == MINION else None
            t = max(targets, key=lambda m: m.attack)
            return (base_minion + 2.0, t, None)
        if kind == "heal":
            if p.hp <= p.max_hp - 8:
                return (base_minion + (p.max_hp - p.hp) * 0.15, p, None)
            return (base_minion, None, None) if c.type == MINION else None
        if kind in ("destroy", "mind_control", "transform"):
            t = biggest([m for m in opp.active_minions if not m.stealth])
            return (minion_value(t), t, None) if t and \
                minion_value(t) >= 7 else None
        if kind == "destroy_damaged":
            targets = [m for m in opp.active_minions
                       if m.damage > 0 and not m.stealth]
            t = biggest(targets)
            return (minion_value(t) + 2, t, None) if t and \
                minion_value(t) >= 5 else None
        if kind == "set_hp1":
            t = biggest([m for m in opp.active_minions
                         if m.health >= 4 and not m.stealth])
            return (t.health * 0.7, t, None) if t else None
        if kind == "freeze":
            t = biggest([m for m in opp.active_minions
                         if not m.frozen and m.attack >= 3])
            if t:
                return (base_minion + 1.5, t, None)
            return (base_minion, None, None) if c.type == MINION else None
        if kind == "sap":
            t = biggest([m for m in opp.active_minions if not m.stealth])
            return (minion_value(t) * 0.8, t, None) if t and \
                minion_value(t) >= 8 else None
        if kind == "surgery":
            t = biggest([m for m in opp.active_minions if not m.stealth])
            return (minion_value(t), t, None) if t and \
                minion_value(t) >= 6 else None
        if kind == "ooze":
            eggs = [m for m in p.active_minions
                    if m.attack == 0 or m.card.name == "The Egg of Khelos"
                    or m.deathrattles]
            if eggs:
                t = max(eggs, key=lambda m: len(m.deathrattles))
                return (base_minion + 2.5, t, None)
            return None
        if kind == "judgment":
            return None  # handled by AoE path
        if kind == "confine":
            own_demons = [m for m in p.active_minions
                          if "DEMON" in m.races]
            if own_demons:
                return (3.0, max(own_demons, key=minion_value), None)
            t = biggest([m for m in opp.active_minions])
            return (minion_value(t) * 0.6, t, None) if t and \
                minion_value(t) >= 9 else None
        if kind == "morbid":
            return None  # choose-one path
        return (base_minion, None, None) if c.type == MINION else None

    def _choose_play(self, game, p, inst):
        c = inst.card
        opp = p.opponent
        cost = p.effective_cost(inst)
        name = c.name
        if name == "Morbid Swarm":
            kill = [m for m in opp.active_minions
                    if m.health <= 4 + p.spell_power]
            if p.corpses >= 2 and kill:
                return (minion_value(max(kill, key=minion_value)) + 1,
                        max(kill, key=minion_value), 1)
            return (2.0, None, 0)
        if name == "Wyvern's Slumber":
            gain = sum(minion_value(m) for m in opp.active_minions
                       if m.health <= 2 + p.spell_power)
            own = sum(minion_value(m) for m in p.active_minions
                      if m.health <= 2 + p.spell_power)
            if gain - own >= 5:
                return (gain - own, None, 1)
            return (cost + 2, None, 0)
        if name == "Secret Ingredient":
            if not p.hero_attacked_turn and (p.weapon or
                                             self.style == "aggro" or
                                             p.hero_attack > 0 or True):
                return (1.5, None, 0)
            return (1.0, None, 1)
        if name == "Twilight Timereaver":
            enemy_atk = sum(m.attack for m in opp.active_minions)
            enemy_hp = sum(m.health for m in opp.active_minions)
            if enemy_hp >= 12 and enemy_hp >= enemy_atk:
                return (cost + enemy_hp * 0.3, None, 0)
            if enemy_atk >= 10:
                return (cost + enemy_atk * 0.3, None, 1)
            return (cost * 0.5, None, 0)
        return (cost + 1, None, 0)

    # -------------------------------------------------------------- attacks
    def do_attack(self, game, p):
        opp = p.opponent
        taunts = [m for m in opp.active_minions
                  if m.taunt and not m.stealth]
        attackers = [m for m in p.active_minions if m.can_attack()]
        for att in sorted(attackers, key=lambda m: -m.attack):
            if game.over:
                return True
            targets = taunts if taunts else \
                [m for m in opp.active_minions if not m.stealth]
            move = self._pick_attack(att, targets, opp, bool(taunts))
            if move is not None:
                game.attack(p, att, move)
                return True
        if p.hero_can_attack() and not game.over:
            move = self._pick_hero_attack(p, opp, taunts)
            if move is not None:
                game.attack(p, "hero", move)
                return True
        return False

    def _pick_attack(self, att, enemy_minions, opp, must_taunt):
        best, best_score = None, 0.0
        for t in enemy_minions:
            if t.dead or t.elusive and False:
                continue
            kills = t.health <= att.attack or att.poisonous
            survives = att.health > t.attack or t.attack == 0 \
                or att.divine_shield or att.immune_attacking
            score = 0.0
            if kills and survives:
                score = minion_value(t) + 3
            elif kills:
                if minion_value(t) > minion_value(att) or t.taunt:
                    score = minion_value(t) - minion_value(att) + 3
            elif must_taunt:
                score = 1.0
            elif survives and t.taunt:
                score = 0.5
            if score > best_score:
                best, best_score = t, score
        if must_taunt:
            return best
        face_ok = att.can_attack_face()
        if self.style == "aggro":
            if best is not None and best_score >= minion_value(best) + 3 \
                    and best.attack >= 3:
                return best
            return opp if face_ok else best
        if self.style == "midrange":
            if best is not None and best_score >= 3:
                return best
            if face_ok:
                return opp
            return best
        if best is not None:
            return best
        return opp if face_ok else None

    def _pick_hero_attack(self, p, opp, taunts):
        safe = p.hp + p.armor > 12 or self.style == "aggro"
        pool = taunts if taunts else list(opp.active_minions)
        kill = [m for m in pool if not m.stealth
                and m.health <= p.hero_attack and m.attack <= 3]
        if kill:
            return max(kill, key=minion_value)
        if taunts:
            weak = [m for m in taunts if m.attack <= 2]
            return max(weak, key=minion_value) if weak and safe else None
        if self.style == "aggro" or safe or p.cls in ("DEMONHUNTER",
                                                      "DRUID"):
            return opp
        return None

    # ----------------------------------------------------------- hero power
    def use_hero_power(self, game, p):
        acted = False
        for which in (p.hero_power, p.hero_power2):
            if which is None or which.passive:
                continue
            if which.used >= which.uses_per_turn or p.mana < which.cost:
                continue
            if which.corpse_cost and p.corpses < which.corpse_cost:
                continue
            cls = p.cls
            name = which.card.name
            opp = p.opponent
            if name == "Life Tap":
                if p.hp > 15 and len(p.hand) < 9 and len(p.deck) > 0:
                    acted = game.use_hero_power(p, which=which) or acted
                continue
            if name == "Fireblast":
                kill = [m for m in opp.active_minions
                        if m.health == 1 and not m.stealth]
                t = max(kill, key=minion_value) if kill else opp
                acted = game.use_hero_power(p, t, which=which) or acted
                continue
            if name in ("Lesser Heal", "Blessing of the Moon"):
                if name == "Lesser Heal":
                    if p.hp < p.max_hp - 3:
                        acted = game.use_hero_power(p, p, which=which) \
                            or acted
                    continue
                acted = game.use_hero_power(p, which=which) or acted
                continue
            if name == "Reinforce" and len(p.board) >= MAX_BOARD:
                continue
            if name in ("Ghoul Charge",) and len(p.board) >= MAX_BOARD:
                continue
            if name == "Dagger Mastery" and p.weapon is not None:
                continue
            if cls not in ("DEMONHUNTER", "DEATHKNIGHT", "ROGUE",
                           "DRUID") and name not in ("Ruthless",) \
                    and p.crystals < 4:
                continue
            acted = game.use_hero_power(p, which=which) or acted
            if acted:
                return True
        return acted
