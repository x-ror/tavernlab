"""Exact-ish lethal search for the current turn.

Answers "can the current player put the opponent to 0 this turn", and
returns a concrete plan.  Covers minion attacks (with taunt clearing),
weapon/hero attacks, hero powers, and direct-damage spells chosen by an
exact mana knapsack.

Two call modes, because the callers have opposite budgets:

* `deep=False` (default, `Agent.try_lethal`): no clones. A scripted game
  is ~2.7 ms and this runs every turn, so the hot path stays allocation
  free and uses the named hero-power table.
* `deep=True` (post-game review, `eval/solvers/lethal.py`): may clone to
  measure an unknown hero power and to try **play-then-lethal** one ply —
  the charge minion or burn spell still sitting in hand. Budget is 20 ms
  per decision point, which buys a lot of clones.

When the taunt search hits its bound it now returns a greedy plan flagged
`approx=True` rather than a silent `None` (design §3.5): "we think this is
lethal but did not prove it" is useful, "no lethal" would be a lie.
"""
from itertools import combinations

from .engine import MAX_BOARD, MINION, SPELL, WEAPON, Minion

# Face damage / hero-attack buff from hero powers, for the no-clone path.
HP_FACE = {"Steady Shot": 2, "Fireblast": 1}
HP_HERO_ATK = {"Demon Claws": 1, "Ruthless": 5, "Shapeshift": 1}

# Probed (face_damage, hero_attack_gain) keyed by hero-power card name.
# A probe costs one clone; the answer is constant for every power we know
# of, so the second call onwards is free. Plans that lean on a *cached*
# probe are marked approx: the measurement came from another board state.
_HP_PROBE = {}

MAX_PLAY_THEN_LETHAL = 6      # candidate cards to try at the extra ply


class Plan(list):
    """A list of actions, plus how much we trust it.

    Subclasses `list` so every existing caller (`execute`, `for act in
    plan`, `plan is None`) keeps working unchanged.
    """

    def __init__(self, acts=(), approx=False, via_play=None):
        super().__init__(acts)
        self.approx = approx
        self.via_play = via_play      # the card played to enable the line

    def describe(self):
        out = []
        for act in self:
            kind = act[0]
            if kind == "spell":
                out.append(f"{act[1].card.name} face")
            elif kind == "play":
                out.append(f"play {act[1].card.name}")
            elif kind == "attack":
                tgt = act[2]
                name = tgt.card.name if isinstance(tgt, Minion) else "face"
                out.append(f"{act[1].card.name} -> {name}")
            elif kind == "hero_attack":
                out.append("hero attack")
            elif kind == "hero_power":
                out.append(f"hero power ({act[2].card.name})")
        return ", ".join(out)


def burn_options(p, deep=False):
    """[(damage, mana_cost, inst)] castable at the enemy hero."""
    out = []
    for inst in p.hand:
        c = inst.card
        if c.type != SPELL:
            continue
        if inst.locked_turn == p.game.turn:
            continue
        if c.target not in ("any", "enemy"):
            continue
        dmg = None
        if c.ai_hint and c.ai_hint[0] == "dmg":
            dmg = c.ai_hint[1] + p.spell_power
        elif deep and c.spell is not None:
            # `ai_hint` is hand-maintained and lags the card pool, so the
            # deep path measures the spell instead of trusting the hint.
            dmg = _probe_spell_face(p, inst)
        if dmg:
            out.append((dmg, p.effective_cost(inst), inst))
    return out


def _probe_spell_face(p, inst):
    """Face damage this spell actually deals, measured on a clone."""
    g = p.game
    try:
        g2 = g.clone()
    except Exception:
        return None
    p2 = g2.players[p.idx]
    i2 = g2.by_eid(inst.eid)
    if i2 is None or i2 not in p2.hand:
        return None
    opp2 = p2.opponent
    before = opp2.hp + opp2.armor
    p2.mana = max(p2.mana, p2.effective_cost(i2))
    try:
        g2.play_card(p2, i2, target=opp2)
    except Exception:
        return None
    dealt = before - (opp2.hp + opp2.armor)
    return dealt if dealt > 0 else None


def best_burn(options, mana):
    """Exact knapsack: max damage within mana. Returns (dmg, [inst])."""
    if not options:
        return 0, []
    best = (0, [])
    n = len(options)
    if n <= 12:
        for r in range(1, n + 1):
            for comb in combinations(options, r):
                cost = sum(c for _, c, _ in comb)
                if cost <= mana:
                    dmg = sum(x[0] for x in comb)
                    if dmg > best[0]:
                        best = (dmg, [i for _, _, i in comb])
    else:
        # Too many burn spells to enumerate exactly: take them by damage
        # per mana. Callers mark the resulting plan approx.
        mana_left, dmg, picked = mana, 0, []
        for d, c, i in sorted(options, key=lambda x: -x[0] / max(1, x[1])):
            if c <= mana_left:
                mana_left -= c
                dmg += d
                picked.append(i)
        best = (dmg, picked)
    return best


def hits_to_break(taunt):
    """Attack hits needed, counting divine shield packets."""
    hits = 1
    if taunt.divine_shield:
        hits += taunt.marks.get("shield_hits", 1)
    return hits


def hero_power_effect(game, p, which, deep=False):
    """(face_damage, hero_attack_gain, from_cache) for one hero power."""
    name = which.card.name
    if name in HP_FACE:
        return HP_FACE[name], 0, False
    if name in HP_HERO_ATK:
        return 0, HP_HERO_ATK[name], False
    if not deep:
        return 0, 0, False
    if name in _HP_PROBE:
        face, atk = _HP_PROBE[name]
        return face, atk, True
    try:
        g2 = game.clone()
    except Exception:
        return 0, 0, False
    p2 = g2.players[p.idx]
    hp2 = g2.by_eid(which.eid)
    if hp2 is None:
        return 0, 0, False
    opp2 = p2.opponent
    before_hp, before_atk = opp2.hp + opp2.armor, p2.hero_attack
    p2.mana = max(p2.mana, hp2.cost)
    hp2.used = 0
    try:
        if not g2.use_hero_power(p2, opp2, which=hp2):
            return 0, 0, False
    except Exception:
        return 0, 0, False
    face = max(0, before_hp - (opp2.hp + opp2.armor))
    atk = max(0, p2.hero_attack - before_atk)
    _HP_PROBE[name] = (face, atk)
    return face, atk, False


def find_lethal(game, p, deep=False):
    """Plan (a `Plan` list) or None.

    Actions: `("spell", inst, target)`, `("attack", minion, target)`,
    `("hero_attack", target)`, `("hero_power", target, hp_state)`,
    `("play", inst, target)`.
    """
    plan = _lethal_now(game, p, deep=deep)
    if plan is not None or not deep:
        return plan
    return _play_then_lethal(game, p)


def _lethal_now(game, p, deep=False):
    opp = p.opponent
    need = opp.hp + opp.armor
    if opp.marks.get("hero_ds"):
        need += 1  # one damage packet is absorbed; approximate
    if opp.immune or need <= 0:
        return None
    taunts = [m for m in opp.active_minions
              if m.taunt and not m.stealth and not m.dead]
    attackers = [m for m in p.active_minions if m.can_attack()]
    swings = []
    for m in attackers:
        remaining = (2 if m.windfury else 1) - m.attacks_done
        for _ in range(max(0, remaining)):
            swings.append(m)
    face_ok = [m for m in attackers if m.can_attack_face()]

    mana = p.mana
    hp_actions = []      # (face_dmg, atk_gain, cost, hp_state, cached)
    for which in (p.hero_power, p.hero_power2):
        if which is None or which.passive or which.used >= which.uses_per_turn:
            continue
        if which.corpse_cost and p.corpses < which.corpse_cost:
            continue
        if mana < which.cost:
            continue
        face, atk, cached = hero_power_effect(game, p, which, deep=deep)
        if face or atk:
            hp_actions.append((face, atk, which.cost, which, cached))

    def hero_swings():
        if p.hero_frozen:
            return 0
        mx = 2 if (p.weapon and p.weapon.windfury) else 1
        return max(0, mx - p.hero_attacks)

    burn_opts = burn_options(p, deep=deep)
    approx_burn = len(burn_opts) > 12

    # ---------------- case 1: no taunts — pure face race
    if not taunts:
        base = sum(m.attack * ((2 if m.windfury else 1) - m.attacks_done)
                   for m in face_ok)
        variants = [(0, 0, 0, None, False)] + hp_actions
        for face_hp, atk_gain, cost, which, cached in variants:
            m_left = mana - cost
            if m_left < 0:
                continue
            atk = p.hero_attack + atk_gain
            swings_n = hero_swings() if atk > 0 else 0
            hero_total = atk * swings_n
            burn_dmg, burn = best_burn(burn_opts, m_left)
            if base + hero_total + burn_dmg + face_hp < need:
                continue
            plan = []
            if which is not None:
                plan.append(("hero_power", opp if face_hp else None, which))
            plan += [("spell", i, opp) for i in burn]
            for m in face_ok:
                for _ in range((2 if m.windfury else 1) - m.attacks_done):
                    plan.append(("attack", m, opp))
            plan += [("hero_attack", opp)] * swings_n
            return Plan(plan, approx=cached or approx_burn)
        return None

    # ---------------- case 2: taunts — clear, then face
    exact = len(taunts) <= 2 and len(swings) <= 9
    hp_face = max([a[0] for a in hp_actions], default=0)
    hp_cost = next((a[2] for a in hp_actions if a[0] == hp_face and hp_face),
                   0)
    hp_state = next((a[3] for a in hp_actions
                     if a[0] == hp_face and hp_face), None)
    hp_cached = any(a[4] for a in hp_actions if a[0] == hp_face and hp_face)
    burn_dmg, burn = best_burn(burn_opts, mana - hp_cost)
    # Unlike the no-taunt branch this does not try hero-attack-buff
    # powers (Demon Claws, Ruthless, Shapeshift): combining a buff with a
    # taunt-clearing assignment needs a second pass over the swings. The
    # error is one-directional — we miss lethals, never invent them —
    # which is the side of the trade a "missed lethal" label can afford.
    hero_total = p.hero_attack * hero_swings()

    def face_damage(face_idx, dead):
        return sum(swings[i].attack for i in face_idx
                   if swings[i].can_attack_face()
                   and swings[i].eid not in dead)

    def finish(used, face_idx, dead, approx):
        plan = [("attack", m, t) for m, t in used]
        plan += [("spell", i, opp) for i in burn]
        if hp_state is not None:
            plan.append(("hero_power", opp, hp_state))
        plan += [("attack", swings[i], opp) for i in face_idx
                 if swings[i].can_attack_face()
                 and swings[i].eid not in dead]
        if p.hero_attack > 0:
            plan += [("hero_attack", opp)] * hero_swings()
        return Plan(plan, approx=approx)

    if exact:
        idxs = list(range(len(swings)))
        for r in range(0, len(idxs) + 1):
            for chosen in combinations(idxs, r):
                got = _assign(swings, chosen, taunts)
                if got is None:
                    continue
                used, dead = got
                face_idx = [i for i in idxs if i not in chosen]
                face_dmg = face_damage(face_idx, dead)
                if face_dmg + hero_total + burn_dmg + hp_face >= need:
                    return finish(used, face_idx, dead,
                                  hp_cached or approx_burn)
        return None

    # Bounded: greedy taunt clear, two orderings (the "1-beam"). Any hit
    # is reported approx — we did not enumerate the alternatives.
    for order in (sorted(range(len(swings)),
                         key=lambda i: swings[i].attack),
                  sorted(range(len(swings)),
                         key=lambda i: -swings[i].attack)):
        chosen = _greedy_clear(swings, order, taunts)
        if chosen is None:
            continue
        got = _assign(swings, chosen, taunts)
        if got is None:
            continue
        used, dead = got
        face_idx = [i for i in range(len(swings)) if i not in chosen]
        face_dmg = face_damage(face_idx, dead)
        if face_dmg + hero_total + burn_dmg + hp_face >= need:
            return finish(used, face_idx, dead, True)
    return None


def _remaining(taunts):
    return {t.eid: [t.health, hits_to_break(t) - 1] for t in taunts}


def _shields(m):
    """Divine-shield packets on one of *our* attackers."""
    if not m.divine_shield:
        return 0
    return m.marks.get("shield_hits", 1)


class _Clear:
    """Bookkeeping for "attack into the taunts, then go face".

    Combat is simultaneous, so a minion sent into a taunt takes the
    taunt's attack back and can die there. Ignoring that was worth two
    false lethals per 150 in the randomised precision run: the plan sent
    a 3/2 into a 5/5 and then counted its 3 damage at the face anyway.
    """

    def __init__(self, taunts):
        self.left = _remaining(taunts)
        self.hp = {}
        self.shield = {}
        self.dead = set()

    def cleared(self):
        return all(hp <= 0 and sh <= 0 for hp, sh in self.left.values())

    def next_taunt(self, taunts):
        return next((t for t in taunts
                     if self.left[t.eid][0] > 0
                     or self.left[t.eid][1] > 0), None)

    def swing(self, m, tgt):
        """Resolve one swing into `tgt`. False if `m` was already dead."""
        if m.eid in self.dead:
            return False
        slot = self.left[tgt.eid]
        if slot[1] > 0:
            slot[1] -= 1                    # popped its divine shield
        else:
            slot[0] -= m.attack
        back = tgt.attack
        sh = self.shield.setdefault(m.eid, _shields(m))
        if sh > 0:
            self.shield[m.eid] = sh - 1     # our shield ate the return
        elif back > 0:
            # Poisonous triggers on damage dealt, so a 0-attack taunt
            # does not kill through it.
            hp = self.hp.setdefault(m.eid, m.health) - back
            self.hp[m.eid] = hp
            if hp <= 0 or tgt.poisonous:
                self.dead.add(m.eid)
        return True


def _assign(swings, chosen, taunts):
    """Assign the chosen swings to taunts.

    Returns `(used, dead_eids)`, or None when the taunts are not all
    cleared or the ordering asks a minion that already died to swing
    again.
    """
    state = _Clear(taunts)
    used = []
    for i in chosen:
        m = swings[i]
        tgt = state.next_taunt(taunts)
        if tgt is None or not state.swing(m, tgt):
            return None
        used.append((m, tgt))
    if not state.cleared():
        return None
    return used, state.dead


def _greedy_clear(swings, order, taunts):
    """Smallest set of swings (in `order`) that clears every taunt."""
    state = _Clear(taunts)
    chosen = []
    for i in order:
        if state.cleared():
            break
        m = swings[i]
        tgt = state.next_taunt(taunts)
        if tgt is None or not state.swing(m, tgt):
            continue
        chosen.append(i)
    if not state.cleared():
        return None
    return chosen


# ---------------------------------------------------------- one extra ply
def _play_then_lethal(game, p):
    """Play one card, then look for lethal again. Deep mode only.

    This is the hole that hides the most real missed lethals: the charge
    minion, the weapon, or the burn spell still in hand when the player
    ended the turn.
    """
    cands = []
    for inst in p.hand:
        c = inst.card
        if inst.locked_turn == game.turn:
            continue
        cost = p.effective_cost(inst)
        if cost > p.mana:
            continue
        if c.corpse_cost and p.corpses < c.corpse_cost:
            continue
        if c.type == MINION:
            if not (c.charge or c.rush) or len(p.board) >= MAX_BOARD:
                continue
        elif c.type == WEAPON:
            if not p.hero_can_attack() and p.hero_attacks:
                continue
        elif c.type == SPELL:
            if c.target not in ("any", "enemy", None):
                continue
        else:
            continue
        cands.append((cost, inst))
    cands.sort(key=lambda x: -x[0])

    for _cost, inst in cands[:MAX_PLAY_THEN_LETHAL]:
        try:
            g2 = game.clone()
        except Exception:
            return None
        p2 = g2.players[p.idx]
        i2 = g2.by_eid(inst.eid)
        if i2 is None or i2 not in p2.hand:
            continue
        try:
            if not g2.play_card(p2, i2, target=p2.opponent):
                continue
        except Exception:
            continue
        if g2.over:
            # The card alone finished it.
            return Plan([("play", inst, p.opponent)], approx=False,
                        via_play=inst.card.name)
        rest = _lethal_now(g2, p2, deep=True)
        if rest is None:
            continue
        plan = Plan([("play", inst, p.opponent)] + list(rest),
                    approx=rest.approx if isinstance(rest, Plan) else True,
                    via_play=inst.card.name)
        return plan
    return None


# ------------------------------------------------------------------ execute
def _live(game, obj):
    """Re-resolve a plan entity by eid.

    A plan produced on a clone addresses the same eids as the original, so
    executing it against the real game just works — except for a minion
    the plan itself summons, which gets a fresh eid here. `execute` falls
    back to matching that one by card name.
    """
    if obj is None:
        return None
    eid = getattr(obj, "eid", 0)
    if not eid:
        return obj
    return game.by_eid(eid) or obj


def execute(game, p, plan):
    opp = p.opponent
    for act in plan:
        if game.over:
            return
        kind = act[0]
        if kind == "play":
            inst = _live(game, act[1])
            if inst in p.hand and p.effective_cost(inst) <= p.mana:
                game.play_card(p, inst, target=opp)
        elif kind == "spell":
            inst = _live(game, act[1])
            if inst in p.hand and p.effective_cost(inst) <= p.mana:
                game.play_card(p, inst, target=opp)
        elif kind == "hero_power":
            # BOTH the power and its target have to be re-resolved. A
            # plan from `_play_then_lethal` was built on a clone, so an
            # un-resolved target is the *clone's* hero — firing at it
            # silently damages the game we were only supposed to be
            # thinking about, and the real opponent never takes the hit.
            which = _live(game, act[2])
            target = _live(game, act[1]) if act[1] is not None else None
            game.use_hero_power(p, target, which=which)
        elif kind == "attack":
            m = _live(game, act[1])
            if m not in p.board:
                # A minion the plan summoned: same card, new eid.
                m = next((x for x in p.active_minions
                          if x.card is act[1].card and x.can_attack()), None)
            tgt = _live(game, act[2])
            if isinstance(tgt, Minion) and (tgt.dead or tgt not in opp.board):
                tgt = opp
            if m is not None and m in p.board and m.can_attack():
                game.attack(p, m, tgt)
        elif kind == "hero_attack":
            if p.hero_can_attack():
                game.attack(p, "hero", opp)
