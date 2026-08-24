"""Hand-written card behaviors, keyed by card id (preferred) or name.

Each entry is a dict of CardDef behavior fields. Registering a card here
marks it implemented.
"""
from .engine import Minion, Location, HeroPowerState, MAX_BOARD, MAX_HAND
from .effects import handler

BEHAVIORS = {}


def B(key, **kw):
    BEHAVIORS[key] = kw
    return kw


def rand_enemy_char(g, p):
    opts = [p.opponent] + [m for m in p.opponent.active_minions
                           if not m.dead]
    return g.rng.choice(opts) if opts else None


def rand_enemy_minion(g, p):
    opts = [m for m in p.opponent.active_minions if not m.dead]
    return g.rng.choice(opts) if opts else None


def summon_n(g, p, name, n=1, dormant=None):
    out = []
    for _ in range(n):
        m = g.summon(p, name, dormant=dormant)
        if m is not None:
            out.append(m)
    return out


# ------------------------------------------------------------ hero powers
def _hp_fireblast(g, p, t):
    g.deal_damage(p, t if t is not None else p.opponent, 1)


def _hp_steady_shot(g, p, t):
    g.deal_damage(p, p.opponent, 2)


def _hp_life_tap(g, p, t):
    g.deal_damage(None, p, 2)
    if not g.over:
        g.draw(p)


def _hp_lesser_heal(g, p, t):
    g.heal(p, t if t is not None else p, 2)


def _hp_armor_up(g, p, t):
    g.gain_armor(p, 2)


def _hp_reinforce(g, p, t):
    g.summon(p, "Silver Hand Recruit")


TOTEM_NAMES = ["Searing Totem", "Stoneclaw Totem", "Healing Totem",
               "Strength Totem"]


def _hp_totemic_call(g, p, t):
    have = {m.card.name for m in p.minions}
    options = [x for x in TOTEM_NAMES if x not in have]
    if options:
        g.summon(p, g.rng.choice(options))


def _hp_dagger(g, p, t):
    from .engine import Weapon
    from .carddata import get_def
    p.weapon = Weapon(get_def("Wicked Knife"))


def _hp_shapeshift(g, p, t):
    p.temp_atk += 1
    g.gain_armor(p, 1)


def _hp_demon_claws(g, p, t):
    p.temp_atk += 1


def _hp_ghoul_charge(g, p, t):
    m = g.summon(p, "Frail Ghoul")
    if m is not None:
        m.charge = True
        m.to_die_eot = True


for name, fn in [("Fireblast", _hp_fireblast), ("Steady Shot",
                 _hp_steady_shot), ("Life Tap", _hp_life_tap),
                 ("Lesser Heal", _hp_lesser_heal), ("Armor Up!",
                 _hp_armor_up), ("Reinforce", _hp_reinforce),
                 ("Totemic Call", _hp_totemic_call),
                 ("Dagger Mastery", _hp_dagger),
                 ("Shapeshift", _hp_shapeshift),
                 ("Demon Claws", _hp_demon_claws),
                 ("Ghoul Charge", _hp_ghoul_charge)]:
    B(name, hero_power_use=fn)


# --------------------------------------------------------- shared tokens
def _healing_totem_eot(g, owner, m, turn_p):
    if turn_p is owner:
        for fm in owner.minions:
            g.heal(None, fm, 1)


B("Healing Totem", triggers={"turn_end": _healing_totem_eot})


def _strength_totem_eot(g, owner, m, turn_p):
    if turn_p is owner:
        others = [x for x in owner.active_minions if x is not m]
        if others:
            g.rng.choice(others).perm_atk += 1


B("Strength Totem", triggers={"turn_end": _strength_totem_eot})
B("Searing Totem")
B("Stoneclaw Totem")
B("Frail Ghoul", **{})
B("The Coin", spell=lambda g, p, t: setattr(
    p, "mana", min(p.mana + 1, 10)))
B("Wicked Knife")
B("Silver Hand Recruit")


# ================================================================ helpers
def get_by_name(name):
    from .carddata import get_def
    return get_def(name)


def add_named(g, p, name, stolen=False, cost_delta=0, temporary=False):
    from .carddata import make_inst_any
    inst = make_inst_any(name)
    inst.cost_delta = cost_delta
    inst.temporary = temporary
    if stolen:
        inst.mark()["stolen"] = True
    return g.add_to_hand(p, inst)


def add_copy_of(g, p, card, stolen=False, cost_delta=0):
    from .engine import CardInst
    inst = CardInst(card)
    inst.cost_delta = cost_delta
    if stolen:
        inst.mark()["stolen"] = True
    return g.add_to_hand(p, inst)


def pool(filt=None):
    from .carddata import standard_pool
    return standard_pool(filt)


def rand_from_pool(g, filt=None):
    opts = pool(filt)
    return g.rng.choice(opts) if opts else None


def discover_pool(g, p, filt=None, ctx=None, stolen=False, cost_delta=0):
    opts = pool(filt)
    pick = g.discover(p, opts, ctx)
    if pick is not None:
        return add_copy_of(g, p, pick, stolen=stolen, cost_delta=cost_delta)
    return None


def discover_from_deck(g, p, filt=None, ctx=None, deck=None):
    """Discover among actual deck card instances; move pick to hand."""
    src = deck if deck is not None else p.deck
    opts = [i for i in src if (filt is None or filt(i.card))]
    if not opts:
        return None
    inst = g.discover(p, opts, ctx)
    if inst is None:
        return None
    src.remove(inst)
    if len(p.hand) < MAX_HAND:
        p.hand.append(inst)
        return inst
    return None


def is_holding(p, pred):
    return any(pred(i.card) for i in p.hand)


def holding_race(p, race):
    return lambda_holding_race(p, race)


def lambda_holding_race(p, race):
    return any(race in i.card.races for i in p.hand)


# ============================================================== Dark Gifts
def _gift_waking(g, p, m):
    m.perm_atk += 3
    m.lifesteal = True


def _gift_bundled(g, p, m):
    m.perm_hp += 4
    m.taunt = True


def _gift_rested(g, p, m):
    m.perm_atk += 2
    m.perm_hp += 2
    m.elusive = True


def _gift_sleepwalker(g, p, m):
    m.charge = True


def _gift_talons(g, p, m):
    m.divine_shield = True
    m.windfury = True


def _gift_reborn(g, p, m):
    m.reborn = True


def _gift_nightmare(g, p, m):
    nm = g.summon(p, m.card)
    if nm is not None:
        nm.atk_base, nm.hp_base = 2, 2
        nm.deathrattles = []
        nm.triggers = {}


def _gift_dreams(g, p, m):
    m.perm_atk += 4
    m.perm_hp += 5


DARK_GIFTS = [_gift_waking, _gift_bundled, _gift_rested, _gift_sleepwalker,
              _gift_talons, _gift_reborn, _gift_nightmare, _gift_dreams]


def random_gift(g):
    return g.rng.choice(DARK_GIFTS)


# ================================================== Herald / Deathwing kit
HERALD_SOLDIER = {
    "ROGUE": "Soldier of Sinestra", "WARLOCK": "Soldier of Cho'gall",
    "SHAMAN": "Soldier of Al'Akir", "DEMONHUNTER": "Soldier of Azshara",
    "WARRIOR": "Soldier of Ragnaros", "DEATHKNIGHT": "Soldier of Onyxia",
}


def herald_scale(p):
    return 4 if p.herald >= 4 else (2 if p.herald >= 2 else 1)


def do_herald(g, p):
    scale = herald_scale(p)
    name = HERALD_SOLDIER.get(p.cls)
    p.herald += 1
    if name is None:
        return
    m = g.summon(p, name)
    if m is not None:
        m.marks["scale"] = scale


def _soldier_sinestra_fx(g, p, m):
    n = m.marks.get("scale", herald_scale(p))
    c = rand_from_pool(g, lambda d: d.type == "SPELL" and
                       d.cls not in (p.cls, "NEUTRAL"))
    if c is not None:
        add_copy_of(g, p, c, cost_delta=-n)


def _soldier_azshara_fx(g, p, m):
    n = 2 * m.marks.get("scale", 1)
    p.temp_atk += n


def _soldier_onyxia_fx(g, p, m):
    n = m.marks.get("scale", 1)
    c = rand_from_pool(g, lambda d: d.type == "MINION" and d.cost == n)
    if c is not None:
        add_copy_of(g, p, c)


def _soldier_ragnaros_dr(g, p, m):
    n = 2 * m.marks.get("scale", 1)
    t = rand_enemy_char(g, p)
    if t is not None:
        g.deal_damage(p, t, n)


def _soldier_alakir_aura(g, p, m):
    n = m.marks.get("scale", 1)
    for x in g.adjacent(m):
        x.aura_atk += n


def _soldier_chogall_eot(g, owner, m, turn_p):
    if turn_p is not owner:
        return
    board = owner.board
    if m in board:
        i = board.index(m)
        if i + 1 < len(board) and isinstance(board[i + 1], Minion):
            n = m.marks.get("scale", 1)
            g.destroy(board[i + 1])
            m.perm_atk += n
            m.perm_hp += n


B("Soldier of Sinestra", on_summon_fx=_soldier_sinestra_fx)
B("Sinestra's Wing", on_summon_fx=_soldier_sinestra_fx)
B("Soldier of Azshara", on_summon_fx=_soldier_azshara_fx)
B("Azshara's Tentacle", on_summon_fx=_soldier_azshara_fx)
B("Soldier of Onyxia", on_summon_fx=_soldier_onyxia_fx)
B("Onyxia's Wing", on_summon_fx=_soldier_onyxia_fx)
B("Soldier of Ragnaros", deathrattle=_soldier_ragnaros_dr)
B("Hand of Ragnaros", deathrattle=_soldier_ragnaros_dr)
B("Soldier of Al'Akir", aura=_soldier_alakir_aura)
B("Charged Hand of Al'Akir", aura=_soldier_alakir_aura)
B("Soldier of Cho'gall", triggers={"turn_end": _soldier_chogall_eot})
B("Cho's Arm", triggers={"turn_end": _soldier_chogall_eot})
B("Gall's Arm", triggers={"turn_end": _soldier_chogall_eot})


def _cata_dragons_reign(g, p):
    g.summon(p, "Progeny of Deathwing")


def _cata_topple(g, p):
    opts = [m for m in p.opponent.active_minions if not m.dead]
    if opts:
        g.destroy(max(opts, key=lambda m: m.health))


def _cata_raze(g, p):
    for m in list(p.opponent.active_minions):
        g.deal_damage(p, m, 4)


def _cata_enthrall(g, p):
    dragons = pool(lambda d: d.type == "MINION" and "DRAGON" in d.races
                   and d.rarity == "LEGENDARY")
    for _ in range(5):
        if dragons:
            c = g.rng.choice(dragons)
            from .engine import CardInst
            inst = CardInst(c)
            inst.cost_delta = -(c.cost - 1)
            p.deck.append(inst)
    g.rng.shuffle(p.deck)


CATACLYSMS = {"Dragon's Reign": _cata_dragons_reign, "Topple": _cata_topple,
              "Raze": _cata_raze, "Enthrall": _cata_enthrall}


def _deathwing_bc(g, p, m, t):
    n = 1 + (1 if p.herald >= 2 else 0) + (1 if p.herald >= 4 else 0)
    picks = g.agents[p.idx].choose_cataclysms(g, p, list(CATACLYSMS), n)
    for name in picks:
        CATACLYSMS[name](g, p)
        g.check_deaths()
        if g.over:
            return
    from .engine import HeroPowerState
    p.hero_power = HeroPowerState(get_by_name("Ruthless"))


def _ruthless(g, p, t):
    p.temp_atk += 5


B("Ruthless", hero_power_use=_ruthless)
B("Deathwing, Worldbreaker", battlecry=_deathwing_bc)
B("Progeny of Deathwing")


def _envoy_bc(g, p, m, t):
    do_herald(g, p)


B("Envoy of the End", battlecry=_envoy_bc)


def _ultraxion_bc(g, p, m, t):
    do_herald(g, p)
    for i in p.hand:
        if i.card.name == "Deathwing, Worldbreaker":
            i.cost_delta -= 2


B("Ultraxion", battlecry=_ultraxion_bc)


# ======================================================= shared neutrals
B("Glacial Shard",
  battlecry=lambda g, p, m, t: t is not None and g.freeze(t),
  battlecry_target="enemy", ai_hint=("freeze",))
B("Rustrot Viper",
  battlecry=lambda g, p, m, t: g.destroy_weapon(p.opponent))
B("Cult Neophyte",
  battlecry=lambda g, p, m, t: setattr(p.opponent, "spell_cost_penalty",
                                       p.opponent.spell_cost_penalty + 1))
B("Fire Fly",
  battlecry=lambda g, p, m, t: add_named(g, p, "Flame Elemental"))
B("Prize Vendor",
  battlecry=lambda g, p, m, t: (g.draw(p), g.draw(p.opponent)),
  deathrattle=lambda g, p, m: (g.draw(p), g.draw(p.opponent)))
B("King Mukla",
  battlecry=lambda g, p, m, t: [add_named(g, p.opponent, "Bananas")
                                for _ in range(2)])
B("Bananas", spell=lambda g, p, t: (isinstance(t, Minion) and not t.dead and
                                    (setattr(t, "perm_atk", t.perm_atk + 1),
                                     setattr(t, "perm_hp", t.perm_hp + 1))),
  target="minion", ai_hint=("buff", 1, 1))


def _curator_bc(g, p, m, t):
    for race in ("BEAST", "DRAGON", "MURLOC"):
        g.draw(p, 1, filt=lambda i, r=race: r in i.card.races)


B("The Curator", battlecry=_curator_bc)


def _eggbearer_bc(g, p, m, t):
    g.draw(p, 1, filt=lambda i: i.card.type == "MINION" and
           i.card.atk == 0)


B("Holy Eggbearer", battlecry=_eggbearer_bc)


def _ooze_bc(g, p, m, t):
    if isinstance(t, Minion) and not t.dead and t.owner is p:
        a, h = t.attack, t.health
        g.destroy(t)
        g.check_deaths()
        inst = add_named(g, p, "Bones")
        if inst is not None:
            inst.mark()["bones"] = (a, h)


def _bones_spell(g, p, t):
    if isinstance(t, Minion) and not t.dead:
        t.perm_atk += 4
        t.perm_hp += 4


B("Dissolving Ooze", battlecry=_ooze_bc,
  battlecry_target="friendly_minion", ai_hint=("ooze",))
B("Bones", spell=_bones_spell, target="friendly_minion",
  ai_hint=("buff", 4, 4))


def _xavius_bc(g, p, m, t):
    pick = discover_from_deck(g, p, lambda c: c.type == "MINION",
                              ctx="minion")
    if pick is not None:
        pick.gift = random_gift(g)


B("Nightmare Lord Xavius", battlecry=_xavius_bc)


def _umbra_bc(g, p, m, t):
    drs = p.graveyard_dr[-5:]
    for card in drs:
        if card.deathrattle:
            card.deathrattle(g, p, m)
            g.check_deaths()
            if g.over:
                return


B("Endbringer Umbra", battlecry=_umbra_bc)


def _remnant_cost(g, p, inst, cost):
    return cost - (p.minions_died_turn + p.opponent.minions_died_turn)


B("Remnant of Rage", cost_fn=_remnant_cost,
  battlecry=lambda g, p, m, t: g.draw(p, 2))


def _dreambound_raptor(g, owner, m, player, played):
    if player is owner and played is not m:
        random_gift(g)(g, owner, played)


B("Dreambound Raptor", triggers={"minion_played": _dreambound_raptor})


def _maiev(g, owner, m, player, played):
    if player is owner and played is not m and not played.dead:
        played.perm_atk += 3
        played.perm_hp += 3
        played.dormant = 2  # wakes at start of owner's next turn


B("Warden Maiev", triggers={"minion_played": _maiev})


def _watfin_bc(g, p, m, t):
    picked = discover_pool(g, p, lambda d: d.type == "MINION",
                           ctx="minion")
    if picked is not None and g.rng.random() < 0.34:
        picked.mark()["watfin"] = True


B("Watfin", battlecry=_watfin_bc)


def _platysaur_bc(g, p, m, t):
    inst = g.draw(p)
    if inst is not None:
        m.marks["held"] = inst


def _platysaur_dr(g, p, m):
    inst = m.marks.get("held")
    if inst is not None and inst in p.hand:
        p.hand.remove(inst)


B("Platysaur", battlecry=_platysaur_bc, deathrattle=_platysaur_dr)


def _briarspawn_eot(g, owner, m, turn_p):
    if turn_p is not owner:
        return
    opts = [x for x in owner.opponent.active_minions if not x.dead]
    if opts:
        t = g.rng.choice(opts)
        excess = max(0, m.attack - t.health)
        g.deal_damage(owner, t, m.attack - excess)
        if excess:
            g.deal_damage(owner, owner.opponent, excess)
    else:
        g.deal_damage(owner, owner.opponent, m.attack)


B("Briarspawn Drake", triggers={"turn_end": _briarspawn_eot})


def _elise_loc_use(g, p, loc, t):
    g.draw(p)
    g.gain_armor(p, 3)


def _elise_bc(g, p, m, t):
    if p.marks.get("start_costs", 0) >= 10:
        from .engine import CardDef, Location, LOCATION
        d = CardDef(id="elise_loc", name="Custom Location", type=LOCATION,
                    cls="NEUTRAL", cost=3, dur=3, races=[], coll=False,
                    text="Draw a card. Gain 3 Armor.", implemented=True,
                    location_use=_elise_loc_use)
        add_copy_of(g, p, d)


B("Elise the Navigator", battlecry=_elise_bc)


def _naralex_hand_cost(g, p, m, inst, cost):
    if "DRAGON" in inst.card.races and \
            not p.marks.get("dragon_played_turn"):
        return min(cost, 1)
    return cost


def _naralex_watch(g, owner, m, player, card):
    if player is owner and "DRAGON" in card.races:
        owner.marks["dragon_played_turn"] = True


def _naralex_ts(g, owner, m, turn_p):
    if turn_p is owner:
        owner.marks["dragon_played_turn"] = False


B("Naralex, Herald of the Flights", on_summon_fx="hand_cost_aura",
  triggers={"hand_cost": _naralex_hand_cost,
            "card_played": _naralex_watch, "turn_start": _naralex_ts})


def _slitherdrake_cost(g, p, inst, cost):
    if sum(1 for i in p.hand
           if i is not inst and "DRAGON" in i.card.races) >= 1:
        return cost - 3
    return cost


B("Prescient Slitherdrake", cost_fn=_slitherdrake_cost)
B("Mother Duck",
  battlecry=lambda g, p, m, t: summon_n(g, p, "Duckling", 3))


def _petal_eot(g, owner, m, turn_p):
    if turn_p is owner:
        opts = [x for x in owner.active_minions
                if x is not m and "DRAGON" in x.races]
        if opts:
            x = g.rng.choice(opts)
            x.perm_atk += 1
            x.perm_hp += 1


B("Petal Peddler", triggers={"turn_end": _petal_eot})
B("Carrier Whelp",
  battlecry=lambda g, p, m, t: (lambda c: c and add_copy_of(g, p, c))(
      rand_from_pool(g, lambda d: d.type == "MINION" and
                     "DRAGON" in d.races and d.cost <= 3)))


def _broodmother_bc(g, p, m, t):
    if lambda_holding_race(p, "DRAGON"):
        p.mana = min(p.mana + 2, p.crystals)


B("Darkscale Broodmother", battlecry=_broodmother_bc)


def _hogger_sog(g, p, inst):
    extra = []
    for i in p.deck:
        if i.card.rarity == "LEGENDARY" and i is not inst:
            extra.append(i.card)
    from .engine import CardInst
    for c in extra:
        p.deck.append(CardInst(c))
    g.rng.shuffle(p.deck)


B("Chainbreaker Hogger", start_of_game=_hogger_sog)


def _twilight_egg_dr(g, p, m):
    n = m.marks.get("whelp", 1)
    w = g.summon(p, "1/1 Token" if False else None) if False else None
    from .engine import CardDef, MINION
    d = CardDef(id="tw_whelp", name="Twilight Whelp", type=MINION,
                cls="NEUTRAL", cost=1, atk=n, hp=n, races=["DRAGON"],
                coll=False, text="", implemented=True)
    g.summon(p, d)


def _twilight_egg_ts(g, owner, m, turn_p):
    if turn_p is owner:
        m.marks["whelp"] = m.marks.get("whelp", 1) + 1


B("Twilight Egg", deathrattle=_twilight_egg_dr,
  triggers={"turn_start": _twilight_egg_ts})


def _hogdriver_bc(g, p, m, t):
    a = g.draw(p)
    b = g.draw(p)
    if a is not None and b is not None and \
            a.card.type == "MINION" and b.card.type == "MINION":
        m.charge = True


B("Getaway Hogdriver", battlecry=_hogdriver_bc)


def _egg_khelos_chain(next_id):
    def dr(g, p, m):
        g.summon(p, next_id)
    return dr


B("DINO_410", deathrattle=_egg_khelos_chain("DINO_410t2"))
B("DINO_410t2", deathrattle=_egg_khelos_chain("DINO_410t3"))
B("DINO_410t3", deathrattle=_egg_khelos_chain("DINO_410t4"))
B("DINO_410t4", deathrattle=_egg_khelos_chain("DINO_410t5"))
B("DINO_410t5", deathrattle=lambda g, p, m: g.summon(p, "DINO_410t"))
B("DINO_410t")


def _timereaver_atk(g, p, m, t):
    for x in p.active_minions + p.opponent.active_minions:
        if x is not m:
            from .autogen import _SYNTH
            x.atk_base = 1
            x.perm_atk = 0
            x.temp_atk = 0
    g.recompute_auras()


def _timereaver_hp(g, p, m, t):
    for x in p.active_minions + p.opponent.active_minions:
        if x is not m:
            x.damage = 0
            x.perm_hp = 0
            x.hp_base = 1
    g.recompute_auras()


B("Twilight Timereaver", choose=(_timereaver_hp, _timereaver_atk))


@handler("accel_aura_tick")
def _accel_aura_tick(g, p, src):
    p.mana = min(p.mana + 1, 10)


def _accel_aura_spell(g, p, t):
    p.at_turn_start("accel_aura_tick", turns=3, repeat=True)


B("Acceleration Aura", spell=_accel_aura_spell)


def _press_adv(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 1)
    p.temp_atk += 1
    g.draw(p)
    g.gain_armor(p, 1)


B("Press the Advantage", spell=_press_adv, target="any",
  ai_hint=("dmg", 1))


def _sands_of_time(g, p, t):
    discover_pool(g, p, lambda d: d.type == "SPELL")


B("Sands of Time", spell=_sands_of_time)


def _tunneling(g, p, m, t):
    pass


B("Tunneling Geomancer")


# ================================================================ Shatter
def find_shatter_halves(full_id):
    """Locate the two 'Shattered' half entities sharing this card's name."""
    from .carddata import DEFS
    full = DEFS[full_id]
    halves = [d for d in DEFS.values()
              if d.name == full.name and d.id != full_id and
              d.text.startswith("Shattered")]
    return sorted(halves, key=lambda d: d.id)


def make_shatterable(full_id, spell_fns, target=None, ai_hint=None):
    """spell_fns: [half1_fn, half2_fn]; full card runs both."""
    def full_spell(g, p, t):
        for fn in spell_fns:
            fn(g, p, t)
            if g.over:
                return
    B(full_id, spell=full_spell, target=target, ai_hint=ai_hint,
      notes="shatter halves playable separately")
    halves = find_shatter_halves(full_id)
    for d, fn in zip(halves, spell_fns):
        BEHAVIORS[d.id] = {"spell": fn, "target": target,
                           "ai_hint": ai_hint}
    return halves


# =============================================================== Burn Mage
def _arcane_barrage(g, p, t):
    tgt = t if t is not None else p.opponent
    g.spell_damage(p, tgt, 3)
    for _ in range(2):
        x = rand_enemy_char(g, p)
        if x is not None and x is not tgt:
            g.deal_damage(p, x, 2 + p.spell_power)
        g.check_deaths()


B("Arcane Barrage", spell=_arcane_barrage, target="enemy",
  ai_hint=("dmg", 3))


def _arcane_flow_a(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 4)


def _arcane_flow_b(g, p, t):
    for m in list(p.opponent.active_minions):
        g.deal_damage(p, m, 2 + p.spell_power)
    g.deal_damage(p, p.opponent, 2 + p.spell_power)


def _kalec_bc(g, p, m, t):
    p.marks["spell_power_bonus"] = p.marks.get("spell_power_bonus", 0) + 1


B("Archmage Kalec", battlecry=_kalec_bc)


def _bookkeeper_dr(g, p, m):
    g.draw(p, 1, filt=lambda i: i.card.type == "SPELL")


def _bookkeeper_kin(g, p, m, t):
    g.summon(p, m.card)


B("Conjured Bookkeeper", deathrattle=_bookkeeper_dr,
  kindred=_bookkeeper_kin)
B("Contraband Wands",
  spell=lambda g, p, t: [add_named(g, p, "Arcane Missiles")
                         for _ in range(3)])


def _arcane_missiles(g, p, t):
    for _ in range(3 + p.spell_power):
        x = rand_enemy_char(g, p)
        if x is not None:
            g.deal_damage(p, x, 1)
        g.check_deaths()


B("Arcane Missiles", spell=_arcane_missiles)


def _frostbolt(g, p, t):
    tgt = t if t is not None else p.opponent
    g.spell_damage(p, tgt, 3)
    if not (isinstance(tgt, Minion) and tgt.dead):
        g.freeze(tgt)


B("Frostbolt", spell=_frostbolt, target="any", ai_hint=("dmg", 3))
B("Living Flame",
  deathrattle=lambda g, p, m: g.draw(
      p, 1, filt=lambda i: i.card.school == "FIRE"))


def _runed_orb(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 2)
    discover_pool(g, p, lambda d: d.type == "SPELL")


B("Runed Orb", spell=_runed_orb, target="any", ai_hint=("dmg", 2))


def _sleet_storm(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 2)
    x = rand_enemy_minion(g, p)
    if x is not None:
        g.deal_damage(p, x, 1 + p.spell_power)


B("Sleet Storm", spell=_sleet_storm, target="any", ai_hint=("dmg", 2))
B("The Skeleton Key",
  spell=lambda g, p, t: discover_pool(g, p, lambda d: d.type == "SPELL"))

MAGE_SECRETS = ["Counterspell", "Ice Barrier"]


def _tricksy_bc(g, p, m, t):
    if any(c.type == "SPELL" for c in p.played_cards_turn):
        for _ in range(2):
            opts = [s for s in MAGE_SECRETS if s not in p.secrets]
            if opts and len(p.secrets) < 5:
                p.secrets.append(g.rng.choice(opts))


B("Tricksy Improviser", battlecry=_tricksy_bc)
B("Violet Spellwing",
  deathrattle=lambda g, p, m: add_named(g, p, "Arcane Missiles"))


# ============================================================ Quest Hunter
def _cower_in_fear(g, p, t):
    if isinstance(t, Minion):
        g.spell_damage(p, t, 3)
    p.marks["next_beast_discount"] = 2


B("Cower in Fear", spell=_cower_in_fear, target="minion",
  ai_hint=("dmg", 3))


def _earthen_roar(g, p, t):
    from .impls import set_hp1
    if isinstance(t, Minion) and not t.dead:
        set_hp1(t)
    if lambda_holding_race(p, "DRAGON"):
        opts = [x for x in p.opponent.active_minions
                if not x.dead and x is not t and x.health >= 3]
        if opts:
            set_hp1(max(opts, key=lambda m: m.health))


def set_hp1(m):
    m.damage = 0
    m.perm_hp = 0
    m.hp_base = 1


B("Earthen Roar", spell=_earthen_roar, target="enemy_minion",
  ai_hint=("set_hp1",))
B("Guard Dog",
  deathrattle=lambda g, p, m: (lambda c: c and g.summon(p, c))(
      rand_from_pool(g, lambda d: d.type == "MINION" and d.cost == 1
                     and d.deathrattle is not None)))
B("Jeweled Macaw",
  battlecry=lambda g, p, m, t: (lambda c: c and add_copy_of(g, p, c))(
      rand_from_pool(g, lambda d: d.type == "MINION"
                     and "BEAST" in d.races)))


@handler("odd_map_echo")
def _odd_map_echo(g, owner, src, player, card, pick_id=None, other_ids=(),
                  token=None):
    if getattr(card, "id", None) != pick_id:
        return
    from .carddata import DEFS
    others = [DEFS[i] for i in other_ids if i in DEFS]
    if others:
        add_copy_of(g, owner, g.rng.choice(others))
    owner.listeners = [l for l in owner.listeners
                       if l.args.get("token") != token]


def _map_discover(g, p, filt, ctx):
    opts = pool(filt)
    if not opts:
        return
    pick = g.discover(p, opts, ctx)
    if pick is None:
        return
    offered = g._last_discover[0]
    inst = add_copy_of(g, p, pick)
    others = [c for c in offered if c is not pick]
    if inst is None or not others:
        return
    token = p.marks["odd_map_n"] = p.marks.get("odd_map_n", 0) + 1
    p.listen("card_played", "odd_map_echo", expiry_turn=g.turn + 1,
             pick_id=pick.id, other_ids=[c.id for c in others],
             token=token)


B("Odd Map",
  spell=lambda g, p, t: _map_discover(
      g, p, lambda d: d.type == "MINION" and "BEAST" in d.races
      and d.atk % 2 == 1, "beast"))


def _pterrorwing_cost(g, p, inst, cost):
    if "BEAST" in p.played_types_last:
        return cost - 2
    return cost


B("Pterrorwing Ravager", cost_fn=_pterrorwing_cost)


def _ravasaur_kin(g, p, m, t):
    x = t if isinstance(t, Minion) and t.owner is p.opponent else \
        rand_enemy_minion(g, p)
    if x is not None:
        g.deal_damage(p, x, m.attack)


B("Ravasaur Matriarch", kindred=_ravasaur_kin,
  battlecry_target="enemy_minion", ai_hint=("dmg", 5))


# Dream cards (classic effects), synthesized: not present in Standard data
def _mk_dream_cards():
    from .engine import CardDef, MINION, SPELL
    from .carddata import DEFS, BY_NAME

    def reg(d):
        DEFS[d.id] = d
        BY_NAME.setdefault(d.name, d.id)

    def dream_spell(g, p, t):
        if isinstance(t, Minion) and not t.dead and \
                t in t.owner.board:
            t.owner.board.remove(t)
            from .engine import CardInst
            if len(t.owner.hand) < MAX_HAND:
                t.owner.hand.append(CardInst(t.card))
            g.recompute_auras()

    def nightmare_spell(g, p, t):
        if isinstance(t, Minion) and not t.dead:
            t.perm_atk += 5
            t.perm_hp += 5
            g.reg(t)
            p.at_turn_start("nightmare_kill", turns=1, target_eid=t.eid)

    def ysera_awakens(g, p, t):
        for x in list(p.active_minions) + list(p.opponent.active_minions):
            g.deal_damage(p, x, 5)
        g.deal_damage(p, p, 5)
        g.deal_damage(p, p.opponent, 5)

    reg(CardDef(id="dream_card", name="Dream", type=SPELL, cls="NEUTRAL",
                cost=0, races=[], coll=False, implemented=True,
                text="Return a minion to its owner's hand.",
                spell=dream_spell, target="minion", ai_hint=("sap",)))
    reg(CardDef(id="dream_nightmare", name="Nightmare", type=SPELL,
                cls="NEUTRAL", cost=0, races=[], coll=False,
                implemented=True,
                text="Give a minion +5/+5. At the start of your next turn, "
                     "destroy it.", spell=nightmare_spell, target="minion",
                ai_hint=("buff", 5, 5)))
    reg(CardDef(id="dream_ysera", name="Ysera Awakens", type=SPELL,
                cls="NEUTRAL", cost=2, races=[], coll=False,
                implemented=True,
                text="Deal 5 damage to all characters.",
                spell=ysera_awakens))
    reg(CardDef(id="dream_drake", name="Emerald Drake", type=MINION,
                cls="NEUTRAL", cost=4, atk=7, hp=6, races=["DRAGON"],
                coll=False, implemented=True, text=""))
    reg(CardDef(id="dream_sister", name="Laughing Sister", type=MINION,
                cls="NEUTRAL", cost=3, atk=3, hp=5, races=[], coll=False,
                implemented=True, elusive=True, text="Elusive"))


_DREAMS = ["Dream", "Nightmare", "Ysera Awakens", "Emerald Drake",
           "Laughing Sister"]


def _shaladrassil(g, p, t):
    for name in _DREAMS:
        add_named(g, p, name)


B("Shaladrassil", spell=_shaladrassil,
  notes="corrupt upgrade not implemented (documented approximation)")


def _zombeast(g, p):
    beasts = pool(lambda d: d.type == "MINION" and "BEAST" in d.races
                  and d.cost <= 5)
    if len(beasts) < 2:
        return None
    a, b = g.rng.sample(beasts, 2)
    from .engine import CardDef, MINION
    kw = {}
    for f in ("taunt", "rush", "charge", "divine_shield", "lifesteal",
              "poisonous", "windfury", "stealth"):
        kw[f] = getattr(a, f) or getattr(b, f)
    d = CardDef(id=f"zb_{a.id}_{b.id}", name="Zombeast", type=MINION,
                cls="HUNTER", cost=max(0, a.cost + b.cost - 3),
                atk=a.atk + b.atk, hp=a.hp + b.hp, races=["BEAST"],
                coll=False, implemented=True, text="", **kw)
    return d


def _storm_gates_reward(g, p):
    d = _zombeast(g, p)
    if d is not None:
        add_copy_of(g, p, d)


def _storm_gates_check(g, p, event, *args):
    if event == "card_played":
        card = args[0]
        if card.type == "MINION" and \
                ("BEAST" in card.races or "UNDEAD" in card.races):
            return 1
    return 0


B("Storm the Gates", quest={"side": True, "target": 3,
                            "check": _storm_gates_check,
                            "reward": _storm_gates_reward})


def _food_chain_check(g, p, event, *args):
    if event == "card_played":
        card = args[0]
        if card.type == "MINION" and "BEAST" in card.races and \
                card.atk in (1, 3, 5, 7):
            seen = p.marks.setdefault("food_chain", set())
            if card.atk not in seen:
                seen.add(card.atk)
                return 1
    return 0


B("The Food Chain", quest={"side": False, "target": 4,
                           "check": _food_chain_check,
                           "reward": lambda g, p: add_named(
                               g, p, "Shokk, Jungle Tyrant")})


def _shokk_bc(g, p, m, t):
    for atk in (8, 6, 4):
        c = discover_pool(g, p, lambda d, a=atk: d.type == "MINION" and
                          "BEAST" in d.races and d.atk == a)
        if c is not None:
            c.cost_delta = -(c.card.cost - 2)


B("Shokk, Jungle Tyrant", battlecry=_shokk_bc)
B("Tracking", spell=lambda g, p, t: discover_from_deck(g, p))


def _underbelly_use(g, p, loc, t):
    from .engine import CardDef, MINION
    d = CardDef(id="ub_rat", name="Rat", type=MINION, cls="HUNTER",
                cost=2, atk=2, hp=1, races=["BEAST"], coll=False,
                implemented=True, text="Deathrattle: Draw a card.",
                deathrattle=lambda g2, p2, m: g2.draw(p2))
    g.summon(p, d)


B("Underbelly Network", location_use=_underbelly_use)


def _wound_prey(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 1)
    from .engine import CardDef, MINION
    d = CardDef(id="wp_hyena", name="Hyena", type=MINION, cls="HUNTER",
                cost=1, atk=1, hp=1, races=["BEAST"], rush=True,
                coll=False, implemented=True, text="Rush")
    g.summon(p, d)


B("Wound Prey", spell=_wound_prey, target="any", ai_hint=("dmg", 1))


# ========================================================= Fatigue Paladin
B("Commander Beatrix")


def _equality(g, p, t):
    for m in list(p.active_minions) + list(p.opponent.active_minions):
        set_hp1(m)
    g.check_deaths()


B("Equality", spell=_equality)


def _hardlight_bc(g, p, m, t):
    g.heal(p, p, 3)
    p.marks["hero_ds"] = True


B("Hardlight Protector", battlecry=_hardlight_bc)


def _judgment(g, p, t):
    if not isinstance(t, Minion) or t.dead or t.owner is not p:
        opts = p.active_minions
        if not opts:
            return
        t = max(opts, key=lambda m: m.attack + m.health)
    a, h = t.attack, t.health
    for m in list(p.active_minions) + list(p.opponent.active_minions):
        m.atk_base, m.perm_atk, m.temp_atk = a, 0, 0
        m.damage = 0
        m.perm_hp = 0
        m.hp_base = h
    g.recompute_auras()
    g.check_deaths()


B("Judgment", spell=_judgment, target="friendly_minion",
  ai_hint=("judgment",))


def _renewing_flames(g, p, t):
    for _ in range(2):
        opts = [x for x in [p.opponent] + list(p.opponent.active_minions)
                if not (isinstance(x, Minion) and x.dead)]
        if not opts:
            return
        low = min(opts, key=lambda x: x.health if isinstance(x, Minion)
                  else x.hp)
        amt = 5 + p.spell_power
        g.deal_damage(p, low, amt)
        g.heal(p, p, amt)
        g.check_deaths()


B("Renewing Flames", spell=_renewing_flames)


def _fins_bc(g, p, m, t):
    from .engine import CardInst
    stash = [g.reg(i) for i in p.hand]
    start = p.marks.get("start_hand", [])
    p.hand = [g.reg(CardInst(c)) for c in start][:MAX_HAND]
    # The stashed instances are now in no zone at all; keeping their eids
    # (not the objects) means Game.clone copies them out of _by_eid.
    p.marks["fins_stash_eids"] = [i.eid for i in stash]


def _fins_eot(g, owner, m, turn_p):
    if turn_p is owner and owner.marks.get("fins_stash_eids") is not None:
        eids = owner.marks.pop("fins_stash_eids")
        back = [g.by_eid(e) for e in eids]
        owner.hand = [i for i in back if i is not None][:MAX_HAND]


B("The Fins Beyond Time", battlecry=_fins_bc,
  triggers={"turn_end": _fins_eot})


def _toreth_aura(g, p, m):
    for x in p.active_minions:
        if x.divine_shield:
            x.marks["shield_hits"] = 3


B("Toreth the Unbreaking", aura=_toreth_aura)


@handler("ursol_cast")
def _ursol_cast(g, p, src, card_id=None):
    from .carddata import DEFS
    d = DEFS.get(card_id)
    fn = d.spell if d is not None else None
    if fn:
        fn(g, p, None)
        g.check_deaths()


def _ursol_bc(g, p, m, t):
    spells = [i for i in p.hand if i.card.type == "SPELL" and i.card.spell]
    if not spells:
        return
    inst = max(spells, key=lambda i: i.card.cost)
    p.hand.remove(inst)
    _ursol_cast(g, p, None, card_id=inst.card.id)
    p.at_turn_start("ursol_cast", turns=2, repeat=True,
                    card_id=inst.card.id)


B("Ursol", battlecry=_ursol_bc)


def _vama_bc(g, p, m, t):
    for x in list(p.active_minions) + list(p.opponent.active_minions):
        if x.card.cls != "PALADIN" and x is not m:
            g.destroy(x)
    g.check_deaths()


B("V'ama, Looming Death", battlecry=_vama_bc)


def _pyro(g, owner, m, caster, card):
    if caster is owner:
        for x in list(owner.active_minions) + \
                list(owner.opponent.active_minions):
            g.deal_damage(owner, x, 1)


B("Wild Pyromancer", triggers={"spell_cast": _pyro})


# ========================================================== Dragon Warrior
def _brood_keeper_bc(g, p, m, t):
    if lambda_holding_race(p, "DRAGON"):
        from .engine import CardDef, WEAPON, Weapon
        d = CardDef(id="bk_sword", name="Sword", type=WEAPON,
                    cls="WARRIOR", cost=2, atk=2, dur=2, races=[],
                    coll=False, implemented=True, text="")
        p.weapon = Weapon(d)


B("Brood Keeper", battlecry=_brood_keeper_bc)


def _darkrider_bc(g, p, m, t):
    if lambda_holding_race(p, "DRAGON"):
        picked = discover_pool(g, p, lambda d: d.type == "MINION" and
                               "DRAGON" in d.races)
        if picked is not None:
            picked.gift = random_gift(g)


B("Darkrider", battlecry=_darkrider_bc)


def _volcano_use(g, p, loc, t):
    n = 3 + (3 if "FIRE" in p.spell_schools_turn else 0)
    for _ in range(n):
        x = rand_enemy_char(g, p)
        if x is not None:
            g.deal_damage(p, x, 1)
        g.check_deaths()
        if g.over:
            return


B("Erupting Volcano", location_use=_volcano_use)


def _sanguine_use(g, p, loc, t):
    if isinstance(t, Minion) and not t.dead:
        g.deal_damage(p, t, 1)
        if not t.dead:
            t.perm_atk += 2


B("Sanguine Depths", location_use=_sanguine_use)


def _searing_fissure(g, p, t):
    for m in list(p.active_minions) + list(p.opponent.active_minions):
        g.deal_damage(p, m, 1 + p.spell_power)
    p.temp_atk += 3


B("Searing Fissure", spell=_searing_fissure)


def _shadowflame_suf(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 2)
    picked = discover_pool(g, p, lambda d: d.type == "MINION" and
                           d.cls == "WARRIOR")
    if picked is not None:
        picked.gift = random_gift(g)


B("Shadowflame Suffusion", spell=_shadowflame_suf, target="any",
  ai_hint=("dmg", 2))


def _announcer_bc(g, p, m, t):
    from .engine import Weapon
    for pl in (p, p.opponent):
        c = rand_from_pool(g, lambda d: d.type == "WEAPON")
        if c is not None:
            pl.weapon = Weapon(c)
    if p.weapon:
        p.weapon.atk += 1
        p.weapon.dur += 1


B("Stadium Announcer", battlecry=_announcer_bc,
  notes="Rewind not used (keep-first-outcome)")


def _torch(g, p, t):
    if not isinstance(t, Minion) or t.damage == 0:
        return
    excess = max(0, 8 - t.health)
    g.deal_damage(p, t, 8)
    if excess > 0:
        add_named(g, p, "Torch")


B("Torch", spell=_torch, target="damaged_enemy_minion",
  ai_hint=("destroy_damaged",),
  notes="excess damage returns a fresh Torch (approximation)")


def _warptooth_count(g, p):
    if g.players[g.current] is not p:
        return
    cnt = p.marks.get("warptooth_dmg", 0) + 1
    p.marks["warptooth_dmg"] = cnt
    if cnt >= 4 and not p.marks.get("warptooth_done"):
        for zone in (p.hand, p.deck):
            for i in list(zone):
                if i.card.name == "Warptooth":
                    zone.remove(i)
                    g.summon(p, i.card)
                    p.marks["warptooth_done"] = True
                    return


@handler("warptooth_hero_dmg")
def _warptooth_hero_dmg(g, owner, src, hero, amount):
    if hero is owner:
        _warptooth_count(g, owner)


@handler("warptooth_minion_dmg")
def _warptooth_minion_dmg(g, owner, src, mo, minion, amount, source):
    if mo is owner:
        _warptooth_count(g, owner)


def _warptooth_sog(g, p, inst):
    p.listen("hero_damaged", "warptooth_hero_dmg")
    p.listen("minion_damaged", "warptooth_minion_dmg")


def _warptooth_ts(g, owner, m, turn_p):
    if turn_p is owner:
        owner.marks["warptooth_dmg"] = 0


B("Warptooth", start_of_game=_warptooth_sog)


def _windpeak_cost(g, p, inst, cost):
    if "DRAGON" in p.played_types_last:
        return cost - 3
    return cost


def _windpeak_bc(g, p, m, t):
    g.deal_damage(p, t if t is not None else p.opponent, 5)
    g.gain_armor(p, 5)


B("Windpeak Wyrm", battlecry=_windpeak_bc, cost_fn=_windpeak_cost,
  battlecry_target="any", ai_hint=("dmg", 5))


# ===================================================== UUB Egg Death Knight
def _thalena_bc(g, p, m, t):
    from .engine import HeroPowerState
    hp2 = HeroPowerState(get_by_name("Ghoul Charge"), cost=0)
    hp2.corpse_cost = 2
    p.hero_power2 = hp2


B("Blood Doctor Thal'ena", battlecry=_thalena_bc,
  notes="second hero power approximated as Ghoul Charge for 2 Corpses")


def _corpse_cannon_atk(g, p):
    g.summon(p, "Frail Ghoul")


B("Corpse Cannon", triggers={"hero_attack": _corpse_cannon_atk})


def _defias_bc(g, p, m, t):
    if isinstance(t, Minion) and not t.dead and t.owner is p:
        t.perm_atk += 2
        t.rush = True


B("Defias Smuggler", battlecry=_defias_bc,
  battlecry_target="friendly_minion", ai_hint=("buff", 2, 0))


def _drink_blood(g, p, t):
    if isinstance(t, Minion):
        amt = 3 + p.spell_power
        g.spell_damage(p, t, 3)
        g.heal(p, p, amt)
    if p.hero_power:
        p.hero_power.used = 0


B("Drink Blood", spell=_drink_blood, target="minion", ai_hint=("dmg", 3))


def _emergency_surgery(g, p, t):
    if not isinstance(t, Minion) or t.dead:
        t = rand_enemy_minion(g, p)
    if t is None:
        return
    from .engine import CardDef, MINION
    d = CardDef(id="es_undead", name="Surgeon", type=MINION,
                cls="DEATHKNIGHT", cost=1, atk=3, hp=1, races=["UNDEAD"],
                lifesteal=True, rush=True, coll=False, implemented=True,
                text="Lifesteal")
    for _ in range(4):
        m = g.summon(p, d)
        if m is None or t.dead or t not in t.owner.board:
            break
        g.attack(p, m, t)
        if g.over:
            return


B("Emergency Surgery", spell=_emergency_surgery, target="enemy_minion",
  ai_hint=("surgery",))


def _falric_bc(g, p, m, t):
    g.draw(p, 1, filt=lambda i: i.card.corpse_cost > 0 or
           "Corpse" in (i.card.text or ""))


B("Falric", battlecry=_falric_bc)


def _infested_breath(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 2)
    g.summon(p, "Bloated Leech")


B("Infested Breath", spell=_infested_breath, target="any",
  ai_hint=("dmg", 2))


def _bloated_leech_eot(g, owner, m, turn_p):
    if turn_p is owner:
        opts = [x for x in [owner.opponent] +
                list(owner.opponent.active_minions)
                if not (isinstance(x, Minion) and x.dead)]
        if opts:
            low = min(opts, key=lambda x: x.health if isinstance(x, Minion)
                      else x.hp)
            g.deal_damage(owner, low, 1)
            g.heal(None, owner, 1)


B("Bloated Leech", triggers={"turn_end": _bloated_leech_eot})


def _morbid_ants(g, p, t):
    from .engine import CardDef, MINION
    d = CardDef(id="ms_ant", name="Ant", type=MINION, cls="DEATHKNIGHT",
                cost=1, atk=1, hp=1, races=["UNDEAD", "BEAST"], coll=False,
                implemented=True, text="")
    summon_n(g, p, d, 2)


def _morbid_dmg(g, p, t):
    if p.corpses >= 2 and isinstance(t, Minion):
        p.corpses -= 2
        g.spell_damage(p, t, 4)


B("Morbid Swarm", choose=(_morbid_ants, _morbid_dmg), target="minion",
  ai_hint=("morbid",))
B("Reanimated Pterrordax", corpse_cost=5,
  notes="costs 5 Corpses instead of mana",
  cost_fn=lambda g, p, inst, cost: 0)


def _sawbones_bc(g, p, m, t):
    others = [x for x in p.active_minions if x is not m]
    n = len(others)
    for x in others:
        g.destroy(x)
    g.check_deaths()
    for _ in range(n):
        g.draw(p)
        p.mana = min(p.mana + 1, p.crystals)


B("Sawbones", battlecry=_sawbones_bc)


def _soulrest(g, p, t):
    for m in p.active_minions:
        m.perm_atk += 1
        m.rush = True
        m.to_die_eot = True


B("Soulrest Ceremony", spell=_soulrest)
B("Staff of the Endbringer",
  deathrattle=lambda g, p, w: [g.destroy(m) for m in
                               list(p.active_minions) +
                               list(p.opponent.active_minions)])


def _tower_dmg(g, owner, m, dmg_owner, minion, amount, source):
    if minion is m:
        summon_n(g, owner, "Frail Ghoul", 2)


B("Tower of Ghouls", triggers={"minion_damaged": _tower_dmg})


# ========================================================== Quest DH
def _axe_cenarius_atk(g, p):
    pass


def _axe_cenarius_after(g, owner, killed):
    pass


def _axe_watch(g, p, killed):
    if killed is not None:
        g.draw(p, 1, filt=lambda i: "Portal to Argus" in i.card.name)


B("Axe of Cenarius", triggers={"hero_attacked":
                               lambda g, p, hero_p, killed:
                               hero_p is p and _axe_watch(g, p, killed)})


def _brox_sog(g, p, inst):
    if inst in p.deck:
        p.deck.remove(inst)
    p.marks["brox_gone"] = inst


def _portal_chain(g, p, num):
    from .engine import CardDef, MINION, CardInst
    names = {1: "TIME_020t2", 2: "TIME_020t3", 3: "TIME_020t4",
             4: "TIME_020t5"}

    def demon_dr(g2, own, m):
        g2.draw(p)
        if num < 4:
            from .carddata import make_inst
            p.deck.append(make_inst(names[num + 1]))
            g2.rng.shuffle(p.deck)
        else:
            brox = p.marks.get("brox_gone")
            if brox is not None:
                brox.cost_delta = -brox.card.cost + 1
                g2.add_to_hand(p, brox)
    d = CardDef(id=f"argus_demon{num}", name="Demon of Argus", type=MINION,
                cls="NEUTRAL", cost=1, atk=num, hp=1, races=["DEMON"],
                coll=False, implemented=True,
                text="", deathrattle=demon_dr)
    g.summon(p.opponent, d)


B("TIME_020t2", spell=lambda g, p, t: _portal_chain(g, p, 1))
B("TIME_020t3", spell=lambda g, p, t: _portal_chain(g, p, 2))
B("TIME_020t4", spell=lambda g, p, t: _portal_chain(g, p, 3))
B("TIME_020t5", spell=lambda g, p, t: _portal_chain(g, p, 4))
B("Broxigar", start_of_game=_brox_sog,
  notes="reappears at 1 mana after 4 Argus demons die")


def _brox_last_stand(g, p, t):
    died = 0
    for m in list(p.active_minions) + list(p.opponent.active_minions):
        hp_before = m.health
        g.deal_damage(p, m, 1 + p.spell_power)
        if m.dead:
            died += 1
    g.check_deaths()
    g.draw(p, died)


B("Broxigar's Last Stand", spell=_brox_last_stand)


def _cosmic(g, p, t):
    def once():
        g.spell_damage(p, t if t is not None else p.opponent, 2)
        c = rand_from_pool(g, lambda d: d.type == "SPELL" and
                           d.cls == "DEMONHUNTER")
        if c is not None:
            from .engine import CardInst
            p.deck.append(CardInst(c))
            g.rng.shuffle(p.deck)
    once()
    if g._outcast:
        once()


B("Cosmic Manifestations", spell=_cosmic, target="any", ai_hint=("dmg", 2))


def _eye_beam(g, p, t):
    if isinstance(t, Minion):
        amt = 3 + p.spell_power
        g.deal_damage(p, t, amt)
        g.heal(p, p, amt)


B("Eye Beam", spell=_eye_beam, target="minion", outcast_discount=2,
  ai_hint=("dmg", 3))


def _felfire_blaze(g, owner, m, caster, card):
    if caster is owner and card.school == "FEL":
        g.destroy(m)
        for x in list(owner.opponent.active_minions):
            g.deal_damage(owner, x, 2)
        g.deal_damage(owner, owner.opponent, 2)


B("Felfire Blaze", triggers={"spell_cast": _felfire_blaze})


def _gorishi_wasp_dmg(g, owner, m, dmg_owner, minion, amount, source):
    if minion is m:
        add_named(g, owner, "Gorishi Stinger")


B("Gorishi Wasp", triggers={"minion_damaged": _gorishi_wasp_dmg})


def _gorishi_stinger(g, p, t):
    g.deal_damage(p, t if t is not None else p.opponent, 2)
    from .engine import CardDef, MINION
    d = CardDef(id="grub", name="Grub", type=MINION, cls="DEMONHUNTER",
                cost=1, atk=2, hp=1, races=["BEAST"], rush=True,
                coll=False, implemented=True, text="Rush")
    g.summon(p, d)


B("Gorishi Stinger", spell=_gorishi_stinger, target="any",
  ai_hint=("dmg", 2))
B("Grim Harvest",
  spell=lambda g, p, t: (g.draw(p), g.summon(
      p, g.rng.choice(["EDR_840t", "EDR_840t1", "EDR_840t2"]))))
B("EDR_840t",
  triggers={"awaken": lambda g, p, m: setattr(p, "temp_atk",
                                              p.temp_atk + 3)})
B("EDR_840t1")
B("EDR_840t2")


def _horn_feasting(g, p, t):
    from .engine import CardDef, MINION
    d = CardDef(id="hf_raptor", name="Raptor", type=MINION,
                cls="DEMONHUNTER", cost=2, atk=2, hp=1, races=["BEAST"],
                rush=True, coll=False, implemented=True, text="Rush")
    ms = summon_n(g, p, d, 3)
    if g._outcast:
        for m in ms:
            m.immune_attacking = True


B("Horn of Feasting", spell=_horn_feasting)


def _illidari_studies(g, p, t):
    picked = discover_pool(g, p, lambda d: "Outcast" in (d.text or ""))
    if picked is not None:
        picked.cost_delta -= 1


B("Illidari Studies", spell=_illidari_studies)
B("Infestation",
  spell=lambda g, p, t: [add_named(g, p, "Gorishi Stinger")
                         for _ in range(2)])
B("Insect Claw",
  triggers={"hero_attack": lambda g, p: (lambda dd: g.summon(p, dd))(
      __import__("hs2.engine", fromlist=["CardDef"]).CardDef(
          id="ic_grub", name="Grub", type="MINION", cls="DEMONHUNTER",
          cost=1, atk=2, hp=1, races=["BEAST"], rush=True, coll=False,
          implemented=True, text="Rush"))})


@handler("irida_tick")
def _irida_tick(g, p, src):
    for _ in range(2):
        if p.void:
            g.add_to_hand(p, p.void.pop(0))


def _irida_bc(g, p, m, t):
    keep = p.deck[-1:] if p.deck else []
    p.void.extend([i for i in p.deck if i not in keep])
    p.deck = list(keep)

    for i in p.void:
        g.reg(i)
    p.at_turn_start("irida_tick", turns=99, repeat=True)


B("Irida Sinseeker", battlecry=_irida_bc)


def _nespirah_use(g, p, loc, t):
    g.deal_damage(p, t if t is not None else p.opponent, 1)


def _nespirah_dr(g, p, loc):
    g.summon(p, "CATA_527t2")


B("Nespirah, Enthralled", location_use=_nespirah_use,
  deathrattle=_nespirah_dr,
  notes="Fel-spell reopen not implemented (approximation)")


def _nespirah_unshackled(g, owner, m, caster, card):
    if caster is owner and card.school == "FEL":
        c = rand_from_pool(g, lambda d: d.type == "MINION" and
                           "NAGA" in d.races)
        if c is not None:
            add_copy_of(g, owner, c, cost_delta=-(c.cost - 1))


B("CATA_527t2", triggers={"spell_cast": _nespirah_unshackled})


@handler("sigil_seas_tick")
def _sigil_seas_tick(g, p, src):
    from .engine import CardDef, MINION
    d = CardDef(id="ss_naga", name="Naga", type=MINION,
                cls="DEMONHUNTER", cost=3, atk=3, hp=3, races=["NAGA"],
                taunt=True, coll=False, implemented=True, text="Taunt")
    g.summon(p, d)


def _sigil_seas(g, p, t):
    p.at_turn_start("sigil_seas_tick", turns=1)


B("Sigil of the Seas", spell=_sigil_seas)


def _colossus_quest_check(g, p, event, *args):
    if event == "damage" and g.players[g.current] is p:
        target, amount = args
        if amount == 2 and (target is p.opponent or
                            (isinstance(target, Minion) and
                             target.owner is p.opponent)):
            return 1
    return 0


B("Unleash the Colossus", quest={"side": False, "target": 12,
                                 "check": _colossus_quest_check,
                                 "reward": lambda g, p: add_named(
                                     g, p, "Gorishi Colossus")})


def _gorishi_colossus_bc(g, p, m, t):
    p.marks["colossus"] = True


B("Gorishi Colossus", battlecry=_gorishi_colossus_bc,
  notes="'deal 2 more on exact 2 damage' handled in engine hook")


def _wyvern_a(g, p, t):
    summon_n(g, p, g.rng.choice(["EDR_840t", "EDR_840t1", "EDR_840t2"]), 2)


def _wyvern_b(g, p, t):
    for m in list(p.active_minions) + list(p.opponent.active_minions):
        g.deal_damage(p, m, 2 + p.spell_power)


B("Wyvern's Slumber", choose=(_wyvern_a, _wyvern_b))


# ========================================================== Attack Druid
def _amirdrassil_use(g, p, loc, t):
    lvl = loc.marks.get("lvl", 0)
    c = rand_from_pool(g, lambda d: d.type == "MINION" and d.cost == 1)
    if c is not None:
        g.summon(p, c)
    g.gain_armor(p, 1 + lvl)
    g.draw(p, 1)
    p.mana = min(p.mana + 1 + lvl, 10)
    loc.marks["lvl"] = lvl + 1


B("Amirdrassil", location_use=_amirdrassil_use,
  notes="improvement approximated as +1 armor/+1 refresh per use")


def _bashana_bc(g, p, m, t):
    for _ in range(3):
        from .engine import CardDef, MINION
        d = CardDef(id="bash_treant", name="Treant", type=MINION,
                    cls="DRUID", cost=1, atk=2, hp=2, races=[], coll=False,
                    implemented=True, text="")
        nm = g.summon(p, d)
        if nm is not None:
            nm.perm_atk += 1
            nm.perm_hp += 1
            nm.taunt = True


B("Bashana Runetotem", battlecry=_bashana_bc,
  notes="carved Nature spells approximated as +1/+1 and Taunt")


def _ebb_flow(g, p, t):
    g.spell_damage(p, t if t is not None else p.opponent, 3)
    inst = getattr(g, "_current_inst", None)
    played = p.marks.get("minions_played", 0)
    base = inst.marks.get("draw_minions", 0) if inst and inst.marks else 0
    if played > base:
        g.gain_armor(p, 5)


B("Ebb and Flow", spell=_ebb_flow, target="any", ai_hint=("dmg", 3))


def _felwood_bc(g, p, m, t):
    p.mana = min(p.mana + 1, 10)


B("Felwood Treant", battlecry=_felwood_bc,
  notes="permanent-crystal upgrade simplified to temporary mana")
B("Horn of Plenty",
  spell=lambda g, p, t: discover_pool(
      g, p, lambda d: d.type == "SPELL" and d.school == "NATURE",
      cost_delta=-2))


def _infest_scullery(g, p, t):
    bonus = min(p.hero_attacks_game // 3, 3)
    for _ in range(2):
        c = rand_from_pool(g, lambda d: d.type == "MINION" and
                           d.cost == 3 + bonus)
        if c is None:
            c = rand_from_pool(g, lambda d: d.type == "MINION" and
                               d.cost == 3)
        if c is not None:
            g.summon(p, c)


B("Infest the Scullery", spell=_infest_scullery,
  notes="improvement: +1 cost tier per 3 hero attacks (approximation)")
B("Innervate", spell=lambda g, p, t: setattr(
    p, "mana", min(p.mana + 1, 10)))


def _lifebloom(g, p, t):
    g.heal(p, p, 8)
    for m in p.active_minions:
        g.heal(p, m, 8)
    for _ in range(2):
        c = rand_from_pool(g, lambda d: d.type == "MINION" and d.cost == 8)
        if c is not None:
            g.summon(p, c)


B("Lifebloom", spell=_lifebloom)


def _merithra_bc(g, p, m, t):
    spent = p.marks.get("mana_spent", 0)
    inst = getattr(g, "_current_inst", None)
    base = inst.marks.get("draw_spend", 0) if inst and inst.marks else 0
    cheap = (spent - base) >= 25
    while len(p.hand) < MAX_HAND:
        c = rand_from_pool(g, lambda d: d.type == "MINION" and
                           "DRAGON" in d.races)
        if c is None:
            break
        add_copy_of(g, p, c,
                    cost_delta=-(c.cost - 1) if cheap else 0)


B("Merithra of the Dream", battlecry=_merithra_bc)


def _secret_ingredient_a(g, p, t):
    p.temp_atk += 2


def _secret_ingredient_b(g, p, t):
    c = rand_from_pool(g, lambda d: d.cls == "DRUID")
    if c is not None:
        add_copy_of(g, p, c)


B("Secret Ingredient", choose=(_secret_ingredient_a, _secret_ingredient_b))


def _spider_rider(g, owner, m, hero_p, killed=None):
    if hero_p is owner:
        g.draw(owner)


B("Spider Rider", triggers={"hero_attacked":
                            lambda g, o, m, hp, killed=None:
                            hp is o and g.draw(o)})


def _spiderling_fx(g, p, m):
    m.marks["hero_atk"] = 1


B("Spiderling", on_summon_fx="hero_atk_aura",
  notes="hero +1 Attack while on board (own turn)")


def _staff_trickery_atk(g, p):
    picked = discover_pool(g, p, lambda d: d.cls == "DRUID")
    if picked is not None:
        picked.cost_delta -= p.hero_attack


B("Staff of Trickery", triggers={"hero_attack": _staff_trickery_atk})


def _waveshaping(g, p, t):
    discover_from_deck(g, p)


B("Waveshaping", spell=_waveshaping,
  notes="'others to bottom' omitted (shuffle-equivalent in sim)")


def _wickerfang_gain(g, owner, m, *args):
    pass


B("Wickerfang", colossal=["CATA_134t3", "CATA_134t3", "CATA_134t3",
                          "CATA_134t3"],
  notes="Legs = 2/2 Treants (approx); stat-share not implemented")
B("CATA_134t3")


# ========================================================== Thief Priest
def _atiesh_fx(g, p):
    pass


def _atiesh_cost(g, p, inst, cost):
    if any(m.card.name == "Medivh the Hallowed"
           for m in p.active_minions):
        return 0
    return cost


B("Atiesh the Greatstaff", cost_fn=_atiesh_cost,
  battlecry=lambda g, p, m, t: p.marks.__setitem__("spell_double", True),
  notes="doubling applied via spell_damage hook while equipped")


def _azalina_sog(g, p, inst):
    p.hp = p.max_hp = 40
    opp = p.opponent
    from .engine import CardInst
    copies = [CardInst(i.card) for i in opp.deck]
    g.rng.shuffle(copies)
    for c in copies[:20]:
        c.mark()["stolen"] = True
        p.deck.append(c)
    g.rng.shuffle(p.deck)


def _azalina_bc(g, p, m, t):
    g.draw(p, MAX_HAND - len(p.hand))


B("Azalina Soulsever", start_of_game=_azalina_sog, battlecry=_azalina_bc)


def _intertwined(g, p, t):
    discover_from_deck(g, p)
    opts = [i.card for i in p.opponent.deck]
    if opts:
        pick = g.discover(p, opts, "steal_deck")
        if pick is not None:
            add_copy_of(g, p, pick, stolen=True)


B("Intertwined Fate", spell=_intertwined)

IMBUE_DISCOUNT = {}


def _imbue(g, p):
    p.marks["imbue"] = p.marks.get("imbue", 0) + 1
    from .engine import HeroPowerState
    if p.hero_power.card.name != "Blessing of the Moon":
        p.hero_power = HeroPowerState(get_by_name("Blessing of the Moon"))


def _blessing_moon(g, p, t):
    lvl = p.marks.get("imbue", 1)
    c = rand_from_pool(g, lambda d: d.cls == "PRIEST" and
                       d.cost <= p.crystals + 2)
    if c is not None:
        add_copy_of(g, p, c, cost_delta=-min(lvl, c.cost))


B("Blessing of the Moon", hero_power_use=_blessing_moon)


def _kaldorei_bc(g, p, m, t):
    for x in p.opponent.active_minions:
        x.temp_atk -= 2
    _imbue(g, p)


B("Kaldorei Priestess", battlecry=_kaldorei_bc,
  notes="-2 Attack lasts one turn cycle (temp_atk)")


def _karazhan_cost(g, p, inst, cost):
    if p.weapon and p.weapon.card.name == "Atiesh the Greatstaff":
        return 0
    return cost


def _karazhan_use(g, p, loc, t):
    for _ in range(2):
        c = rand_from_pool(g, lambda d: d.type == "MINION" and d.cost == 8)
        if c is not None:
            g.summon(p, c)


B("Karazhan the Sanctum", cost_fn=_karazhan_cost,
  location_use=_karazhan_use)
B("Lunarwing Messenger", battlecry=lambda g, p, m, t: _imbue(g, p))


def _medivh_cost(g, p, inst, cost):
    if any(l.card.name == "Karazhan the Sanctum" for l in p.locations):
        return 0
    return cost


def _medivh_bc(g, p, m, t):
    for x in list(p.active_minions) + list(p.opponent.active_minions):
        if x is not m:
            x.silence()
            g.destroy(x)
    g.check_deaths()


B("Medivh the Hallowed", cost_fn=_medivh_cost, battlecry=_medivh_bc)


def _mind_sweeper_bc(g, p, m, t):
    inst = getattr(g, "_current_inst", None)
    played = p.marks.get("opp_copies_played", 0)
    base = inst.marks.get("draw_copies", 0) if inst and inst.marks else 0
    if played > base:
        for x in list(p.opponent.active_minions):
            g.deal_damage(p, x, 2)


B("Mind Sweeper", battlecry=_mind_sweeper_bc)


def _moonwell(g, p, t):
    for x in list(p.opponent.active_minions):
        g.deal_damage(p, x, 4 + p.spell_power)
    g.deal_damage(p, p.opponent, 4 + p.spell_power)
    g.heal(p, p, 4)
    for x in p.active_minions:
        g.heal(p, x, 4)


B("Moonwell", spell=_moonwell)


def _sw_ruin(g, p, t):
    for x in list(p.active_minions) + list(p.opponent.active_minions):
        if x.attack >= 5:
            g.destroy(x)
    g.check_deaths()


B("Shadow Word: Ruin", spell=_sw_ruin)


def _soothsayer_dr(g, p, m):
    g.heal(None, p, 6)
    c = rand_from_pool(g, lambda d: d.type == "MINION" and d.cost == 6)
    if c is not None:
        g.summon(p, c)


B("Soothsayer", deathrattle=_soothsayer_dr)


def _unshackle_cost(g, p, inst, cost):
    played = p.marks.get("opp_copies_played", 0)
    base = inst.marks.get("draw_copies", 0) if inst.marks else 0
    if played > base:
        return 1
    return cost


B("Unshackle Soul", target="minion", ai_hint=("destroy",),
  cost_fn=_unshackle_cost,
  spell=lambda g, p, t: isinstance(t, Minion) and g.destroy(t))


# ========================================================== Herald Rogue
def _agent_bc(g, p, m, t):
    if not p.hand:
        return
    worst = min(p.hand, key=lambda i: i.card.cost if i.card.cost > 3
                else 10 - i.card.cost)
    idx = p.hand.index(worst)
    p.hand.remove(worst)
    add_named(g, p, "The Coin")


B("Agent of the Old Ones", battlecry=_agent_bc,
  notes="AI transforms its least useful card into a Coin")
B("Cultist Map",
  spell=lambda g, p, t: discover_from_deck(g, p),
  notes="'play this turn -> extra pick' simplified away")


def _deja_vu(g, p, t):
    opts = [i.card for i in p.opponent.hand]
    if opts:
        pick = g.discover(p, opts, "steal_hand")
        if pick is not None:
            add_copy_of(g, p, pick, stolen=True)


B("Deja Vu", spell=_deja_vu)


def _garona_bc(g, p, m, t):
    opp = p.opponent
    for i in list(opp.hand):
        if i.card.name == "King Llane":
            opp.hand.remove(i)
            opp.hp = max(1, opp.hp // 2)
            return


B("Garona Halforcen", battlecry=_garona_bc)


def _llane_sog(g, p, inst):
    if inst in p.deck:
        p.deck.remove(inst)
        p.opponent.deck.append(inst)
        g.rng.shuffle(p.opponent.deck)


def _llane_bc(g, p, m, t):
    g.draw(p)
    from .engine import CardInst
    p.deck.append(CardInst(m.card))
    g.rng.shuffle(p.deck)


B("King Llane", start_of_game=_llane_sog, battlecry=_llane_bc)
B("Lotus Bookie",
  deathrattle=lambda g, p, m: add_named(g, p, "The Coin"))
B("Maniacal Follower",
  deathrattle=lambda g, p, m: do_herald(g, p))


def _mirrex_bc(g, p, m, t):
    last = p.opponent.marks.get("last_minion")
    if last is not None:
        m.card = last
        m.atk_base, m.hp_base = 3, 4
        m.deathrattles = [last.deathrattle] if last.deathrattle else []
        m.triggers = dict(last.triggers)
        m.aura = last.aura
        g.recompute_auras()


B("Mirrex, the Crystalline", battlecry=_mirrex_bc,
  notes="copies the last enemy minion as 3/4 when played")


def _nightmare_fuel(g, p, t):
    opts = [i.card for i in p.opponent.deck if i.card.type == "MINION"]
    if opts:
        pick = g.discover(p, opts, "steal_deck_minion")
        if pick is not None:
            return add_copy_of(g, p, pick, stolen=True)
    return None


def _nightmare_fuel_combo(g, p, t):
    inst = _nightmare_fuel(g, p, t)
    if inst is not None:
        inst.gift = random_gift(g)


B("Nightmare Fuel", spell=_nightmare_fuel,
  combo_spell=_nightmare_fuel_combo)


def _fan_of_knives(g, p):
    for x in list(p.opponent.active_minions):
        g.deal_damage(p, x, 1 + p.spell_power)
    g.draw(p)


def _opu_fx(g, p, m, t=None):
    _fan_of_knives(g, p)


B("Opu the Unseen", battlecry=_opu_fx, combo=_opu_fx,
  deathrattle=lambda g, p, m: _fan_of_knives(g, p))
B("Preparation", spell=lambda g, p, t: setattr(p, "spell_discount", 2))


def _rite_twilight(g, p, t):
    do_herald(g, p)


def _rite_twilight_combo(g, p, t):
    do_herald(g, p)
    g.spell_damage(p, t if t is not None else p.opponent, 3)


B("Rite of Twilight", spell=_rite_twilight,
  combo_spell=_rite_twilight_combo, target="any", ai_hint=("dmg", 3))


@handler("shadow_demise_watch")
def _shadow_demise_watch(g, owner, src, caster, card):
    if src is not None and caster is owner and src in owner.hand and \
            card.name != "Shadow of Demise" and card.type == "SPELL":
        src.card = card


def _shadow_demise_sog(g, p, inst):
    g.reg(inst)
    p.listen("spell_cast", "shadow_demise_watch", source_eid=inst.eid)


B("Shadow of Demise", start_of_game=_shadow_demise_sog,
  spell=lambda g, p, t: None)


def _sinestra_fx(g, p, m):
    p.marks["cast_twice_other_cls"] = True


B("Sinestra", colossal=["CATA_154t", "CATA_154t1"],
  on_summon_fx=_sinestra_fx,
  notes="'spells from other classes cast twice' persists once summoned")
B("CATA_154t1", on_summon_fx=_soldier_sinestra_fx)


def _kingslayers_atk(g, p):
    for pl in (p, p.opponent):
        g.draw(pl, 1, filt=lambda i: i.card.rarity == "LEGENDARY")


B("The Kingslayers", triggers={"hero_attack": _kingslayers_atk})


def _twilight_mistress_bc(g, p, m, t):
    opp = p.opponent
    from .engine import CardInst
    for x in list(opp.active_minions):
        if x in opp.board:
            opp.board.remove(x)
            if len(opp.hand) < MAX_HAND:
                opp.hand.append(CardInst(x.card))
    g.recompute_auras()


B("Twilight Mistress", battlecry=_twilight_mistress_bc)


# ============================================================ Zee Shaman
def _alakir_bc(g, p, m, t):
    for _ in range(2):
        c = rand_from_pool(g, lambda d: d.type == "MINION" and
                           d.cost == m.attack)
        if c is not None:
            add_copy_of(g, p, c, cost_delta=-(c.cost - 1))


B("Al'Akir, Lord of Storms", battlecry=_alakir_bc,
  colossal=["CATA_153t", "CATA_153t1"])


def _gallagio(g, owner, m, player, played):
    if player is owner and played is not m and played.card.battlecry \
            and not played.dead:
        played.perm_atk += 1
        played.perm_hp += 1


B("Gallagio Goon", triggers={"minion_played": _gallagio})


def _securitybot_bc(g, p, m, t):
    for x in p.active_minions:
        if x is not m:
            x.perm_atk += 1
            x.perm_hp += 1


B("Hijacked Securitybot", battlecry=_securitybot_bc)


def _mugzee_sog(g, p, inst):
    minions = sum(1 for i in p.deck
                  if i.card.type == "MINION" and i is not inst)
    spells = sum(1 for i in p.deck if i.card.type == "SPELL")
    if spells == 0:
        p.marks["zee"] = True
    if minions == 0:
        p.marks["mug"] = True


def _zee_watch(g, owner, m):
    if owner.marks.get("zee"):
        n = owner.marks.get("zee_count", 0) + 1
        owner.marks["zee_count"] = n
        if n % 5 == 0:
            m.marks["bc_twice"] = True


B("Mug'Zee", start_of_game=_mugzee_sog,
  notes="Zee's Might implemented; Mug's Magic not needed for this deck")


def _tiny_pal_bc(g, p, m, t):
    p.weapon.marks["ammo"] = "fire"


def _tiny_pal_atk(g, p):
    x = rand_enemy_minion(g, p)
    if x is not None:
        g.deal_damage(p, x, 2)


B("Tiny Pal", battlecry=_tiny_pal_bc,
  triggers={"hero_attack": _tiny_pal_atk},
  notes="elemental ammo approximated as 2 dmg to a random enemy minion")
B("Witch's Apprentice",
  battlecry=lambda g, p, m, t: (lambda c: c and add_copy_of(g, p, c))(
      rand_from_pool(g, lambda d: d.type == "SPELL" and
                     d.cls == "SHAMAN")))
B("Skywall Sentinel", battlecry=lambda g, p, m, t: do_herald(g, p))


# ========================================================= Herald Warlock
def _annihilation(g, p, t):
    for x in list(p.active_minions) + list(p.opponent.active_minions):
        g.destroy(x)
    g.check_deaths()
    bottom = p.deck[:3]
    for i in list(bottom):
        if i.card.type == "MINION" and "DEMON" in i.card.races:
            p.deck.remove(i)
            g.summon(p, i.card)


B("Annihilation", spell=_annihilation)


def _caged_cranium_bc(g, p, m, t):
    m.perm_hp += len(p.hand)


B("Caged Cranium", battlecry=_caged_cranium_bc)


def _conflagrate(g, p, t):
    if isinstance(t, Minion):
        g.spell_damage(p, t, 5)
        g.draw(t.owner)


B("Conflagrate", spell=_conflagrate, target="minion", ai_hint=("dmg", 5))


def _cursed_catacombs(g, p, t):
    inst = discover_from_deck(g, p)
    if inst is not None:
        inst.temporary = True


B("Cursed Catacombs", spell=_cursed_catacombs)


@handler("nightmare_kill")
def _nightmare_kill(g, p, src, target_eid=None):
    t = g.by_eid(target_eid)
    if t is not None:
        g.destroy(t)
        g.check_deaths()


@handler("cursed_chains_return")
def _cursed_chains_return(g, p, src, target_eid=None, owner_eid=None):
    t = g.by_eid(target_eid)
    opp = g.by_eid(owner_eid)
    if t is None or opp is None or t.dead or t not in p.board:
        return
    p.board.remove(t)
    if len(opp.board) < MAX_BOARD:
        t.owner = opp
        t.cant_attack = t.card.cant_attack
        opp.board.append(t)
    g.recompute_auras()


def _cursed_chains(g, p, t):
    if not isinstance(t, Minion) or t.dead:
        return
    opp = t.owner
    if t in opp.board and len(p.board) < MAX_BOARD:
        opp.board.remove(t)
        t.owner = p
        t.cant_attack = True
        p.board.append(t)
        g.recompute_auras()
        g.reg(t)
        p.at_turn_start("cursed_chains_return", turns=1,
                        target_eid=t.eid, owner_eid=opp.eid)


B("Cursed Chains", spell=_cursed_chains, target="enemy_minion",
  ai_hint=("mind_control",),
  notes="'until end of their turn' approximated to your next turn start")


def _demonic_confinement(g, p, t):
    if not isinstance(t, Minion) or t.dead:
        return
    if t.owner is p and "DEMON" in t.races:
        t.perm_atk += 3
        t.perm_hp += 3
    else:
        t.dormant = 2


B("Demonic Confinement", spell=_demonic_confinement, target="minion",
  ai_hint=("confine",))
B("Drain Soul", target="minion", ai_hint=("dmg", 3),
  spell=lambda g, p, t: isinstance(t, Minion) and
  (lambda amt: (g.deal_damage(p, t, amt), g.heal(p, p, amt)))(
      3 + p.spell_power))


@handler("godfrey_overdraw")
def _godfrey_overdraw(g, owner, src, over_p, burned):
    if over_p is not owner:
        return
    burned.cost_delta -= 1
    g.reg(burned)
    # eids, not instances: marks must survive a clone (design 4.2)
    owner.marks.setdefault("overflow_eids", []).append(burned.eid)


@handler("godfrey_tick")
def _godfrey_tick(g, p, src):
    keep = []
    for e in p.marks.get("overflow_eids", []):
        inst = g.by_eid(e)
        if inst is None:
            continue
        if len(p.hand) < MAX_HAND:
            p.hand.append(inst)
        else:
            keep.append(e)
    p.marks["overflow_eids"] = keep


def _godfrey_sog(g, p, inst):
    p.listen("overdraw", "godfrey_overdraw")
    p.at_turn_start("godfrey_tick", turns=99, repeat=True)


B("Godfrey the Betrayer", start_of_game=_godfrey_sog)


def _imp_gang_dr(g, p, m):
    from .engine import CardDef, MINION, CardInst
    d = CardDef(id="igs_demon", name="Sightless Watcher", type=MINION,
                cls="WARLOCK", cost=8, atk=8, hp=8, races=["DEMON"],
                taunt=True, lifesteal=True, coll=False, implemented=True,
                text="Taunt, Lifesteal")
    for _ in range(2):
        p.deck.insert(0, CardInst(d))


B("Imp Gang Stooge", deathrattle=_imp_gang_dr)


@handler("rotten_apple_hurt")
def _rotten_apple_hurt(g, p, src):
    g.deal_damage(None, p, 3)


def _rotten_apple(g, p, t):
    g.heal(p, p, 12)
    p.at_turn_start("rotten_apple_hurt", turns=2, repeat=True)


B("Rotten Apple", spell=_rotten_apple)
B("Shadowsworn Disciple",
  battlecry=lambda g, p, m, t: do_herald(g, p),
  deathrattle=lambda g, p, m: g.heal(None, p, 3))


def _shrine_use(g, p, loc, t):
    do_herald(g, p)
    g.draw(p)


B("Shrine of Twilight", location_use=_shrine_use)


def _spire_use(g, p, loc, t):
    n = len(p.hand)
    from .engine import CardDef, MINION
    d = CardDef(id="spire_demon", name="Void Demon", type=MINION,
                cls="WARLOCK", cost=5, atk=n, hp=n, races=["DEMON"],
                coll=False, implemented=True, text="")
    m = g.summon(p, d)
    if m is not None:
        x = rand_enemy_minion(g, p)
        if x is not None:
            g.attack(p, m, x)


B("Spire of Solitude", location_use=_spire_use)


def _unseen_atlas_cost(g, p, inst, cost):
    return cost - len(p.hand)


B("The Unseen Atlas", cost_fn=_unseen_atlas_cost,
  spell=lambda g, p, t: g.draw(p, 3))


# ============================================================== post-build
B("Arcane Flow", spell=lambda g, p, t: (_arcane_flow_a(g, p, t),
                                        _arcane_flow_b(g, p, t)),
  target="any", ai_hint=("dmg", 4))


@handler("zee_pre_battlecry")
def _zee_pre_battlecry(g, owner, src, player, m):
    if player is owner:
        _zee_watch(g, owner, m)


def _wire_zee(g, p):
    p.listen("pre_battlecry", "zee_pre_battlecry")


# patch Mug'Zee start-of-game to wire Zee listener
_old_mugzee = _mugzee_sog


def _mugzee_sog2(g, p, inst):
    _old_mugzee(g, p, inst)
    if p.marks.get("zee"):
        _wire_zee(g, p)
    if p.marks.get("mug"):
        def ts(g2, owner, turn_p):
            pass
BEHAVIORS["Mug'Zee"]["start_of_game"] = _mugzee_sog2


def post_build(DEFS, BY_NAME):
    """Runs after all CardDefs exist: synthesize missing tokens."""
    from .engine import CardDef, MINION, SPELL

    def reg(d):
        DEFS[d.id] = d
        BY_NAME.setdefault(d.name, d.id)

    _mk_dream_cards()
    if "Bananas" not in BY_NAME:
        reg(CardDef(id="bananas", name="Bananas", type=SPELL,
                    cls="NEUTRAL", cost=1, races=[], coll=False,
                    implemented=True, text="Give a minion +1/+1.",
                    target="minion", ai_hint=("buff", 1, 1),
                    spell=lambda g, p, t: (isinstance(t, Minion) and
                                           not t.dead and
                                           (setattr(t, "perm_atk",
                                                    t.perm_atk + 1),
                                            setattr(t, "perm_hp",
                                                    t.perm_hp + 1)))))
    if "Flame Elemental" not in BY_NAME:
        reg(CardDef(id="flame_ele", name="Flame Elemental", type=MINION,
                    cls="NEUTRAL", cost=1, atk=1, hp=2,
                    races=["ELEMENTAL"], coll=False, implemented=True,
                    text=""))
    if "Frail Ghoul" not in BY_NAME:
        reg(CardDef(id="frail_ghoul", name="Frail Ghoul", type=MINION,
                    cls="DEATHKNIGHT", cost=1, atk=1, hp=1,
                    races=["UNDEAD"], coll=False, implemented=True,
                    charge=True, text="Charge. Dies at end of turn."))
    if "Ghoul Charge" not in BY_NAME:
        reg(CardDef(id="ghoul_charge_hp", name="Ghoul Charge",
                    type="HERO_POWER", cls="DEATHKNIGHT", cost=2,
                    races=[], coll=False, implemented=True,
                    text="Summon a 1/1 Ghoul with Charge. "
                         "It dies at end of turn.",
                    hero_power_use=_hp_ghoul_charge))
    if "Arcane Missiles" not in BY_NAME:
        reg(CardDef(id="arcane_missiles", name="Arcane Missiles",
                    type=SPELL, cls="MAGE", cost=1, races=[], coll=False,
                    implemented=True, school="ARCANE",
                    text="Deal 3 damage randomly split among all enemies.",
                    spell=_arcane_missiles))
    # mark every registered behavior's def implemented + frail ghoul EOT
    fg = DEFS[BY_NAME["Frail Ghoul"]]
    fg.charge = True


def _hellfire(g, p, t):
    for x in list(p.active_minions) + list(p.opponent.active_minions):
        g.deal_damage(p, x, 3 + p.spell_power)
    g.deal_damage(p, p, 3 + p.spell_power)
    g.deal_damage(p, p.opponent, 3 + p.spell_power)


B("Hellfire", spell=_hellfire)


# ==================================================== Raza DH extra cards
def _dark_bribe(g, p, t):
    drawn = []
    for _ in range(3):
        inst = g.draw(p)
        if inst is not None:
            drawn.append(inst)
    if drawn:
        give = min(drawn, key=lambda i: i.card.cost)
        if give in p.hand:
            p.hand.remove(give)
            g.add_to_hand(p.opponent, give)


B("Dark Bribe", spell=_dark_bribe,
  notes="AI gives away its cheapest drawn card")


def _roach_hp(g, owner, m, hp_player):
    if hp_player is owner:
        owner.mana = min(owner.mana + 2, owner.crystals)


B("Enduring Roach", triggers={"hero_power_used": _roach_hp})


def _eredar_draw(g, owner, m, drawer, inst):
    if drawer is owner:
        from .engine import CardDef, MINION
        d = CardDef(id="eredar_imp", name="Ravenous Felhunter",
                    type=MINION, cls="DEMONHUNTER", cost=1, atk=1, hp=1,
                    races=["DEMON"], rush=True, coll=False,
                    implemented=True, text="Rush")
        g.summon(owner, d)


B("Eredar Deceptor", triggers={"card_drawn": _eredar_draw})


def _fumigate(g, p, t):
    if not isinstance(t, Minion) or t.dead:
        return
    races = set(t.races)
    g.spell_damage(p, t, 3)
    for x in list(p.active_minions) + list(p.opponent.active_minions):
        if x is not t and not x.dead and races & set(x.races):
            g.deal_damage(p, x, 3 + p.spell_power)


B("Fumigate", spell=_fumigate, target="minion", ai_hint=("dmg", 3))
B("Royal Librarian",
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                t.silence()),
  battlecry_target="minion", ai_hint=("silence",))


def _collapsing_star(g, p, t):
    x = rand_enemy_char(g, p)
    if x is not None:
        g.deal_damage(p, x, 2 + p.marks.get("cstar_bonus", 0))


@handler("collapsing_star_refresh")
def _collapsing_star_refresh(g, owner, src, sp, minion):
    if sp is owner and "DEMON" in minion.races and owner.hero_power \
            and owner.hero_power.card.name == "Collapsing Star":
        owner.hero_power.used = 0


def _soul_immolation(g, p, t):
    from .engine import HeroPowerState
    if p.hero_power and p.hero_power.card.name == "Collapsing Star":
        p.marks["cstar_bonus"] = p.marks.get("cstar_bonus", 0) + 1
    else:
        p.hero_power = g.reg(
            HeroPowerState(get_by_name("Collapsing Star")))
        p.listen("summon", "collapsing_star_refresh")


B("Soul Immolation", spell=_soul_immolation)
B("Collapsing Star", hero_power_use=_collapsing_star)


# ==================================================== Burn Mage (hsreplay)
def _first_flame(g, p, t):
    if isinstance(t, Minion):
        g.spell_damage(p, t, 2)
    add_named(g, p, "Second Flame")


B("First Flame", spell=_first_flame, target="minion", ai_hint=("dmg", 2))


def _raincaller_hit(g, owner, m, caster, dealt):
    if caster is owner and not m.marks.get("rc_done"):
        m.marks["rc_done"] = True
        m.perm_atk += 2


def _raincaller_ts(g, owner, m, turn_p):
    if turn_p is owner:
        m.marks["rc_done"] = False


B("Raincaller", triggers={"spell_dealt_damage": _raincaller_hit,
                          "turn_start": _raincaller_ts})


def _smoldering_grove(g, p, t):
    inst = getattr(g, "_current_inst", None)
    held = 0
    if inst is not None and inst.marks:
        held = max(0, (g.turn - inst.marks.get("draw_turn", g.turn)) // 2)
    g.draw(p, min(1 + held, 3))


B("Smoldering Grove", spell=_smoldering_grove,
  notes="upgrade +1 draw per turn held (cap 3); discard clause omitted")


def _brilliance_cost(g, p, inst, cost):
    return cost - p.marks.get("spell_dmg_turn", 0)


def _brilliance(g, p, t):
    from .engine import CardDef, MINION
    d = CardDef(id="sb_dragon", name="Spellwoven Dragon", type=MINION,
                cls="MAGE", cost=6, atk=6, hp=6, races=["DRAGON"],
                coll=False, implemented=True, text="")
    g.summon(p, d)


B("Spellweaver's Brilliance", spell=_brilliance, cost_fn=_brilliance_cost)


def _seer_aura(g, p, m):
    m.spell_dmg = 2 if m.damage > 0 else 0


B("Time-Twisted Seer", aura=_seer_aura)


def _unstable_bc(g, p, m, t):
    if p.marks.get("spell_dmg_turn", 0) > 0:
        g.summon(p, m.card)


B("Unstable Spellcaster", battlecry=_unstable_bc)


def _vulcanos_eot(g, owner, m, turn_p):
    if turn_p is owner:
        for x in list(owner.active_minions) + \
                list(owner.opponent.active_minions):
            if x is not m:
                g.deal_damage(owner, x, 3)
        g.check_deaths()


B("Vulcanos", colossal=["CATA_488t", "CATA_488t2"],
  triggers={"turn_end": _vulcanos_eot})


def _plume_dmg(g, owner, m, dmg_owner, minion, amount, source):
    if minion is m:
        c = rand_from_pool(g, lambda d: d.type == "SPELL" and
                           d.school == "FIRE")
        if c is not None:
            add_copy_of(g, owner, c, cost_delta=-3)


B("Plume of Vulcanos", triggers={"minion_damaged": _plume_dmg})
B("Winterspring Whelp",
  battlecry=lambda g, p, m, t: discover_pool(
      g, p, lambda d: d.type == "SPELL" and d.cost == 1))


def _post_build_mage(DEFS, BY_NAME):
    from .engine import CardDef, SPELL

    def reg(d):
        DEFS[d.id] = d
        BY_NAME.setdefault(d.name, d.id)
    if "Second Flame" not in BY_NAME:
        reg(CardDef(id="second_flame", name="Second Flame", type=SPELL,
                    cls="MAGE", cost=1, races=[], coll=False,
                    implemented=True, school="FIRE",
                    text="Deal 5 damage to a minion.", target="minion",
                    ai_hint=("dmg", 5),
                    spell=lambda g, p, t: isinstance(t, Minion) and
                    g.spell_damage(p, t, 5)))


_old_post_build = post_build


def post_build(DEFS, BY_NAME):
    _old_post_build(DEFS, BY_NAME)
    _post_build_mage(DEFS, BY_NAME)
