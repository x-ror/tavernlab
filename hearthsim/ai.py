"""Heuristic agent: lethal detection (incl. Force of Nature + Savage Roar /
Bloodlust combos), value-based card plays, targeted removal discipline,
favorable trading, archetype-dependent face aggression."""
from .engine import Minion, MINION, SPELL, WEAPON, MAX_BOARD
from .cards import get_card

AURA_MIDDLE = {"Flametongue Totem", "Dire Wolf Alpha", "Defender of Argus",
               "Sunfury Protector"}
BURN = {  # direct damage usable at face: name -> base damage
    "Fireball": 6, "Frostbolt": 3, "Pyroblast": 10, "Eviscerate": 2,
    "Lightning Bolt": 3, "Lava Burst": 5, "Soulfire": 4, "Darkbomb": 3,
    "Kill Command": 3, "Hammer of Wrath": 3, "Holy Fire": 5,
}


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

    # ------------------------------------------------------------ main loop
    def take_turn(self, game, p):
        for _ in range(80):
            if game.over:
                return
            if self.try_lethal(game, p):
                return
            if self.play_best_card(game, p):
                continue
            if self.use_hero_power(game, p):
                continue
            if self.do_attack(game, p):
                continue
            break

    # --------------------------------------------------------------- lethal
    def face_burn(self, p, extra_mana=0):
        """Greedy: max face damage from hand spells with available mana."""
        mana = p.mana + extra_mana
        total = 0
        sp = p.spell_power
        cards = []
        options = []
        for c in p.hand:
            if c.name in BURN:
                dmg = BURN[c.name] + sp
                if c.name == "Kill Command" and any(
                        m.tribe == "beast" for m in p.board):
                    dmg = 5 + sp
                options.append((dmg, p.effective_cost(c), c))
        options.sort(key=lambda x: -x[0] / max(1, x[1]))
        for dmg, cost, c in options:
            if cost <= mana:
                mana -= cost
                total += dmg
                cards.append(c)
        return total, cards

    def try_lethal(self, game, p):
        opp = p.opponent
        if any(m.taunt and not m.stealth for m in opp.board):
            return False
        attackers = [m for m in p.board if m.can_attack_face()]
        base = sum(m.attack for m in attackers)
        hero_atk = p.hero_attack if p.hero_can_attack() else 0
        burn, burn_cards = self.face_burn(p)
        need = opp.hp + opp.armor

        plan = None
        if base + hero_atk + burn >= need:
            plan = ("plain", burn_cards)
        else:
            # Savage Roar / Bloodlust / Force of Nature combos
            for combo in self._combo_plans(game, p, attackers, hero_atk):
                if combo[0] >= need:
                    plan = ("combo", combo[1])
                    break
        if not plan:
            return False

        if plan[0] == "combo":
            for c in plan[1]:
                if c in p.hand and p.effective_cost(c) <= p.mana:
                    game.play_card(p, c)
        else:
            for c in plan[1]:
                if c in p.hand and p.effective_cost(c) <= p.mana:
                    game.play_card(p, c, target=opp)
        for m in list(p.board):
            if m.can_attack_face() and not game.over:
                game.attack(p, m, opp)
        if p.hero_can_attack() and not game.over:
            game.attack(p, "hero", opp)
        if not game.over and plan[0] == "combo":
            burn2, cards2 = self.face_burn(p)
            for c in cards2:
                if not game.over and p.effective_cost(c) <= p.mana:
                    game.play_card(p, c, target=opp)
        return True

    def _combo_plans(self, game, p, attackers, hero_atk):
        plans = []
        mana = p.mana
        roar = next((c for c in p.hand if c.name == "Savage Roar"), None)
        fon = next((c for c in p.hand if c.name == "Force of Nature"), None)
        lust = next((c for c in p.hand if c.name == "Bloodlust"), None)
        n_att = len(attackers)
        base = sum(m.attack for m in attackers)
        if roar and p.effective_cost(roar) <= mana:
            plans.append((base + 2 * n_att + hero_atk + 2, [roar]))
        if lust and p.effective_cost(lust) <= mana:
            plans.append((base + 3 * n_att + hero_atk, [lust]))
        if roar and fon:
            cost = p.effective_cost(fon) + p.effective_cost(roar)
            if cost <= mana:
                treants = min(3, MAX_BOARD - len(p.board))
                plans.append((base + treants * 4 + 2 * n_att + hero_atk + 2,
                              [fon, roar]))
        elif fon and p.effective_cost(fon) <= mana:
            treants = min(3, MAX_BOARD - len(p.board))
            plans.append((base + treants * 2 + hero_atk, [fon]))
        plans.sort(key=lambda x: -x[0])
        return plans

    # ---------------------------------------------------------- card plays
    DRAW_CARDS = {"Shiv", "Fan of Knives", "Arcane Intellect",
                  "Loot Hoarder", "Gadgetzan Auctioneer", "Shield Block",
                  "Slam", "Hammer of Wrath", "Power Word: Shield",
                  "Azure Drake", "Chaos Strike", "Skull of Gul'dan",
                  "Mana Tide Totem", "Acolyte of Pain"}

    def play_best_card(self, game, p):
        best = None
        low_deck = len(p.deck) <= 3
        for c in list(p.hand):
            if p.effective_cost(c) > p.mana:
                continue
            if c.play_if and not c.play_if(game, p):
                continue
            if low_deck and c.name in self.DRAW_CARDS and len(p.hand) > 2:
                continue
            res = self.evaluate_play(game, p, c)
            if res is None:
                continue
            score, target, choice = res
            if score <= 0:
                continue
            if best is None or score > best[0]:
                best = (score, c, target, choice)
        if best is None:
            return False
        score, c, target, choice = best
        pos = None
        if c.type == MINION and c.name in AURA_MIDDLE and len(p.board) >= 2:
            pos = len(p.board) // 2
        game.play_card(p, c, target=target, choice=choice, position=pos)
        return True

    def evaluate_play(self, game, p, c):
        """Returns (score, target, choice) or None if the card should wait."""
        opp = p.opponent
        name = c.name
        style = self.style
        cost = p.effective_cost(c)

        # --- special-cased cards -------------------------------------
        if name == "The Coin" or name == "Innervate":
            gain = 1 if name == "The Coin" else 2
            for other in p.hand:
                if other is c or other.name in ("The Coin", "Innervate"):
                    continue
                ec = p.effective_cost(other)
                if p.mana < ec <= p.mana + gain:
                    return (0.5, None, None)
            return None
        if name in ("Counterspell", "Mirror Entity", "Ice Barrier",
                    "Ice Block", "Noble Sacrifice", "Explosive Trap",
                    "Freezing Trap", "Snake Trap"):
            if c.secret in p.secrets:
                return None
            return (1.0, None, None)
        if name == "Wild Growth":
            return (6.0 if p.crystals <= 7 else 0.2, None, None)
        if name == "Preparation":
            nxt = [x for x in p.hand
                   if x.type == SPELL and x.cost >= 3 and x is not c]
            return (0.4, None, None) if nxt else None
        if name == "Deadly Poison":
            return (3.0, None, None) if p.weapon else None
        if name == "Blade Flurry":
            if not p.weapon:
                return None
            dmg = p.weapon.atk
            kills = sum(1 for m in opp.board if m.health <= dmg)
            return (kills * 3.0 - 1.5, None, None) if kills >= 2 else None
        if name == "Unleash the Hounds":
            n = len(opp.board)
            return (n * 1.5, None, None) if n >= 3 else None
        if name in ("Savage Roar", "Bloodlust", "Force of Nature"):
            return None  # only used by lethal planner
        if name == "Brawl":
            if len(opp.board) - len(p.board) >= 2 and len(opp.board) >= 3:
                return (10.0, None, None)
            return None
        if name == "Equality":
            ov = sum(minion_value(m) for m in opp.board)
            mv = sum(minion_value(m) for m in p.board)
            return (8.0, None, None) if ov - mv >= 8 else None
        if name == "Frost Nova":
            threat = sum(m.attack for m in opp.board)
            return (4.0, None, None) if threat >= 8 else None
        if name == "Circle of Healing":
            auch = any(m.card.name == "Auchenai Soulpriest"
                       for m in p.board)
            if auch:
                kills = sum(1 for m in opp.board if m.health <= 4)
                own = sum(1 for m in p.board if m.health <= 4)
                return (kills * 3.0 - own * 2.0, None, None) \
                    if kills >= 2 else None
            healed = sum(min(m.damage, 4) for m in p.board)
            return (healed * 0.5, None, None) if healed >= 4 else None
        if name == "Mind Control":
            t = biggest(opp.board)
            if t and minion_value(t) >= 8 and len(p.board) < MAX_BOARD:
                return (14.0, t, None)
            return None
        if name == "Alexstrasza":
            tgt = p if p.hp <= 12 else opp
            if tgt is opp and opp.hp <= 15:
                tgt = p if p.hp < 15 else None
            if tgt is None:
                return None
            return (12.0, tgt, None)
        if name == "Faceless Manipulator":
            t = biggest(list(p.board) + list(opp.board))
            if t and minion_value(t) >= 10:
                return (minion_value(t), t, None)
            return None
        if name == "Big Game Hunter":
            t = biggest([m for m in opp.board if m.attack >= 7])
            return (13.0, t, None) if t else None
        if name == "Sylvanas Windrunner" and len(p.board) >= MAX_BOARD:
            return None
        if name == "Twilight Drake":
            return (cost + len(p.hand) * 0.4, None, None)
        if name == "Doomguard":
            penalty = max(0, len(p.hand) - 1) * 0.8
            return (max(0.5, 8.0 - penalty), None, None)
        if name == "Soulfire":
            res = self._damage_target(game, p, 4, allow_face=True)
            if res is None:
                return None
            s, t = res
            return (s - len(p.hand) * 0.5, t, None)

        # --- AoE by expected value -----------------------------------
        aoe = {"Whirlwind": (1, True), "Consecration": (2, False),
               "Holy Nova": (2, False), "Flamestrike": (4, False),
               "Blizzard": (2, False), "Fan of Knives": (1, False),
               "Lightning Storm": (2, False), "Hellfire": (3, True),
               "Chaos Nova": (4, True), "Arcane Missiles": (1, False)}
        if name in aoe:
            dmg, hits_own = aoe[name]
            dmg += p.spell_power
            gain = sum(min(minion_value(m), minion_value(m) + 2)
                       for m in opp.board if m.health <= dmg)
            gain += sum(1 for m in opp.board if m.health > dmg)
            if hits_own:
                gain -= sum(minion_value(m)
                            for m in p.board if m.health <= dmg)
            threshold = 4.0 if style != "control" else 3.0
            if name == "Fan of Knives" and gain < threshold:
                return (0.3, None, None) if len(p.hand) <= 4 else None
            return (gain, None, None) if gain >= threshold else None

        # --- targeted effects via ai_hint ----------------------------
        if c.ai_hint:
            return self._hint_play(game, p, c)

        # --- choose-one minions/spells -------------------------------
        if c.choose:
            return self._choose_play(game, p, c)

        # --- weapons -------------------------------------------------
        if c.type == WEAPON:
            if p.weapon and p.weapon.atk * p.weapon.dur >= c.atk * c.dur:
                return None
            return (cost + 1.0, None, None)

        # --- plain minions & spells ----------------------------------
        if c.type == MINION:
            if len(p.board) >= MAX_BOARD:
                return None
            return (cost + 1.0 + (1.0 if c.charge or c.taunt else 0),
                    None, None)
        if c.type == SPELL:
            if c.target is None:
                return (cost * 0.8 + 0.3, None, None)
            return None
        return None

    # ------------------------------------------------- targeting helpers
    def _damage_target(self, game, p, n, allow_face=False, minion_only=False,
                       undamaged_only=False, friendly=False):
        opp = p.opponent
        pool = p.board if friendly else opp.board
        kill = [m for m in pool if m.health <= n and not m.dead
                and not m.stealth
                and not (undamaged_only and m.damage > 0)]
        if kill:
            t = max(kill, key=minion_value)
            waste = n - t.health
            return (minion_value(t) + 2 - waste * 0.3, t)
        if allow_face and not minion_only:
            if self.style != "control" or opp.hp <= n * 2:
                return (n * 0.45, opp)
        big = [m for m in pool if not m.dead and not m.stealth
               and m.health > n and minion_value(m) >= 8
               and not (undamaged_only and m.damage > 0)]
        if big and self.style != "aggro":
            t = max(big, key=minion_value)
            return (1.0, t)
        return None

    def _hint_play(self, game, p, c):
        opp = p.opponent
        hint = c.ai_hint
        kind = hint[0]
        cost = p.effective_cost(c)
        base_minion = (cost + 1.0) if c.type == MINION else 0.0

        if kind == "dmg":
            n = hint[1] + (p.spell_power if c.type == SPELL else 0)
            if c.name == "Kill Command" and any(
                    m.tribe == "beast" for m in p.board):
                n = 5 + p.spell_power
            undam = c.target == "undamaged_minion"
            res = self._damage_target(
                game, p, n, allow_face=(c.target == "any"),
                minion_only=c.target in ("minion", "enemy_minion",
                                         "undamaged_minion"),
                undamaged_only=undam)
            if res is None:
                if c.type == MINION and len(p.board) < MAX_BOARD:
                    return (base_minion, None, None)
                return None
            s, t = res
            return (s + base_minion, t, None)

        if kind in ("buff", "divine_shield", "temp_atk"):
            targets = [m for m in p.board
                       if not m.dead and m.card.name != c.name]
            if kind == "temp_atk":
                targets = [m for m in targets if m.can_attack()]
            if not targets:
                if c.type == MINION and len(p.board) < MAX_BOARD:
                    return (base_minion * 0.8, None, None)
                return None
            t = max(targets, key=lambda m: m.attack)
            bonus = 2.0 if kind != "temp_atk" else 1.0
            if c.name == "Power Word: Shield":
                bonus = 2.5
            return (base_minion + bonus, t, None)

        if kind == "silence":
            targets = [m for m in opp.board if not m.dead and
                       (m.taunt or m.attack >= 4 or m.deathrattles or
                        m.aura or m.triggers) and not m.silenced]
            if not targets:
                return None
            return (base_minion + 3.0, max(targets, key=minion_value), None)

        if kind == "silence_dmg":
            kill = [m for m in opp.board if m.health <= 1 + p.spell_power]
            good = [m for m in opp.board if not m.silenced and
                    (m.taunt or m.perm_atk + m.perm_hp >= 3
                     or m.deathrattles)]
            pool = kill + good
            if not pool:
                return None
            return (3.0, max(pool, key=minion_value), None)

        if kind == "heal":
            if p.hp <= 20:
                return (base_minion + (30 - p.hp) * 0.15, p, None)
            hurt = [m for m in p.board if m.damage >= 2]
            if hurt:
                return (base_minion + 1.0, max(hurt, key=minion_value), None)
            return (base_minion, None, None) if c.type == MINION else None

        if kind == "destroy_big":
            t = biggest([m for m in opp.board if m.attack >= 7])
            return (13.0, t, None) if t else None
        if kind == "destroy_small":
            targets = [m for m in opp.board
                       if m.attack <= 3 and not m.dead and not m.stealth]
            t = biggest(targets)
            return (minion_value(t), t, None) if t and minion_value(t) >= 5 \
                else None
        if kind == "destroy_big5":
            t = biggest([m for m in opp.board
                         if m.attack >= 5 and not m.stealth])
            return (minion_value(t) + 2, t, None) if t else None
        if kind == "destroy":
            t = biggest([m for m in opp.board if not m.stealth])
            return (minion_value(t), t, None) if t and minion_value(t) >= 7 \
                else None
        if kind == "destroy_damaged":
            targets = [m for m in opp.board
                       if m.damage > 0 and not m.dead and not m.stealth]
            t = biggest(targets)
            return (minion_value(t) + 2, t, None) if t and \
                minion_value(t) >= 5 else None
        if kind == "transform":
            t = biggest([m for m in opp.board if not m.stealth])
            thresh = 9 if c.name == "Polymorph" else 8
            return (minion_value(t) + 1, t, None) if t and \
                minion_value(t) >= thresh else None
        if kind == "sap":
            taunts = [m for m in opp.board if m.taunt and not m.stealth]
            t = biggest(taunts) or biggest(
                [m for m in opp.board if not m.stealth])
            return (minion_value(t) * 0.8, t, None) if t and \
                minion_value(t) >= 8 else None
        if kind == "set_atk1":
            t = biggest([m for m in opp.board
                         if m.attack >= 4 and not m.stealth])
            if t:
                return (base_minion + t.attack * 0.6, t, None)
            return (base_minion * 0.7, None, None)
        if kind == "set_hp1":
            t = biggest([m for m in opp.board
                         if m.health >= 4 and not m.stealth])
            return (t.health * 0.7, t, None) if t else None
        if kind == "shield_slam":
            kill = [m for m in opp.board
                    if m.health <= p.armor + p.spell_power
                    and not m.stealth]
            t = biggest(kill)
            return (minion_value(t) + 1, t, None) if t and \
                minion_value(t) >= 5 else None
        if kind == "taskmaster":
            kill = [m for m in opp.board if m.health == 1 and not m.stealth]
            if kill:
                return (base_minion + 2, max(kill, key=minion_value), None)
            own = [m for m in p.board if m.health >= 2]
            if own:
                return (base_minion + 1, max(own, key=lambda m: m.attack),
                        None)
            return (base_minion * 0.7, None, None)
        if kind == "rockbiter":
            if p.weapon or self.style == "aggro":
                return (2.0, p, None)
            kill = [m for m in opp.board if m.health <= 3]
            if kill and not p.hero_frozen and p.hero_attacks == 0:
                return (3.0, p, None)
            return None
        if kind == "mind_control" or kind == "copy_big" \
                or kind == "alexstrasza" or kind == "keeper":
            pass  # handled by name above / choose logic
        if kind == "keeper":
            pass
        return (base_minion, None, None) if c.type == MINION else None

    def _choose_play(self, game, p, c):
        opp = p.opponent
        cost = p.effective_cost(c)
        name = c.name
        if name == "Wrath":
            n = 3 + p.spell_power
            res = self._damage_target(game, p, n, minion_only=True)
            if res:
                return (res[0], res[1], 0)
            res = self._damage_target(game, p, 1 + p.spell_power,
                                      minion_only=True)
            if res:
                return (res[0] + 1.0, res[1], 1)
            return None
        if name == "Keeper of the Grove":
            kill = [m for m in opp.board
                    if m.health <= 2 and not m.stealth]
            if kill:
                return (cost + 3, max(kill, key=minion_value), 0)
            sil = [m for m in opp.board if not m.silenced and
                   (m.taunt or m.attack >= 5 or m.deathrattles)]
            if sil:
                return (cost + 3, max(sil, key=minion_value), 1)
            return (cost + 0.5, opp, 0)
        if name == "Druid of the Claw":
            kill = [m for m in opp.board if m.health <= 4]
            if self.style != "control" and (kill or opp.hp <= 12) \
                    and p.hp > 12:
                return (cost + 2, None, 0)
            return (cost + 2, None, 1)
        if name == "Nourish":
            if p.crystals <= 6:
                return (5.0, None, 0)
            return (4.0, None, 1)
        if name == "Ancient of Lore":
            if p.hp <= 12:
                return (cost + 2, None, 1)
            return (cost + 2, None, 0)
        if name == "Ancient of War":
            return (cost + 2, None, 0)
        return (cost + 1, None, 0)

    # -------------------------------------------------------------- attacks
    def do_attack(self, game, p):
        opp = p.opponent
        taunts = [m for m in opp.board if m.taunt and not m.stealth]
        attackers = [m for m in p.board if m.can_attack()]
        if not attackers and not p.hero_can_attack():
            return False

        for att in sorted(attackers, key=lambda m: -m.attack):
            if game.over:
                return True
            targets = taunts if taunts else \
                [m for m in opp.board if not m.stealth]
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
            if t.dead:
                continue
            kills = t.health <= att.attack or att.poisonous
            survives = att.health > t.attack or t.attack == 0 \
                or att.divine_shield
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
            # take clean kills, otherwise push face
            if best is not None and best_score >= 3:
                return best
            if face_ok:
                return opp
            return best
        # control: trade whenever possible, face otherwise
        if best is not None:
            return best
        return opp if face_ok else None

    def _pick_hero_attack(self, p, opp, taunts):
        safe = p.hp + p.armor > 12 or self.style == "aggro"
        pool = taunts if taunts else list(opp.board)
        kill = [m for m in pool if not m.stealth
                and m.health <= p.hero_attack and m.attack <= 3]
        if kill:
            return max(kill, key=minion_value)
        if taunts:
            weak = [m for m in taunts if m.attack <= 2]
            return max(weak, key=minion_value) if weak and safe else None
        if p.cls == "Demon Hunter" or self.style == "aggro" or safe:
            return opp
        return None

    # ----------------------------------------------------------- hero power
    def use_hero_power(self, game, p):
        if p.hp_used:
            return False
        cost = 1 if p.cls == "Demon Hunter" else 2
        if p.mana < cost:
            return False
        opp = p.opponent
        c = p.cls
        if c == "Warlock":
            if p.hp > 15 and len(p.hand) < 9 and len(p.deck) > 0:
                return game.use_hero_power(p)
            return False
        if c == "Mage":
            kill = [m for m in opp.board if m.health == 1 and not m.stealth]
            t = max(kill, key=minion_value) if kill else opp
            return game.use_hero_power(p, t)
        if c == "Priest":
            if p.hp < 26:
                return game.use_hero_power(p, p)
            hurt = [m for m in p.board if m.damage > 0]
            if hurt:
                return game.use_hero_power(p, max(hurt, key=minion_value))
            if p.hp < 30:
                return game.use_hero_power(p, p)
            return False
        if c == "Paladin" and len(p.board) >= MAX_BOARD:
            return False
        if c == "Demon Hunter":
            return game.use_hero_power(p)
        if c == "Rogue":
            if p.weapon is None:
                return game.use_hero_power(p)
            return False
        # avoid wasting mana early for slow classes
        if p.crystals < 4:
            return False
        return game.use_hero_power(p)
