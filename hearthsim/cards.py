"""Card database: Basic/Classic pool + well documented Naxx/GvG staples
and the Demon Hunter Initiate set. Every card's mechanic is implemented
faithfully within the engine's rules."""
from .engine import CardDef, MINION, SPELL, WEAPON, Minion, Weapon, MAX_BOARD

DB = {}


def C(*args, **kw):
    card = CardDef(*args, **kw)
    DB[card.name] = card
    return card


def get_card(name):
    return DB[name]


# ------------------------------------------------------------------ helpers
def enemy_chars(p):
    return [p.opponent] + list(p.opponent.board)


def rand_enemy_char(g, p):
    opts = [c for c in enemy_chars(p)
            if not (isinstance(c, Minion) and c.dead)]
    return g.rng.choice(opts) if opts else None


def dmg_spell(n):
    def fn(g, p, t):
        g.spell_damage(p, t if t is not None else p.opponent, n)
    return fn


def aoe_enemy_minions(n, freeze=False):
    def fn(g, p, t):
        amt = n + p.spell_power
        for m in list(p.opponent.board):
            g.deal_damage(p, m, amt)
            if freeze and not m.dead:
                g.freeze(m)
    return fn


def draw_n(n):
    def fn(g, p, t):
        g.draw(p, n)
    return fn


def buff_spell(a, h):
    def fn(g, p, t):
        if isinstance(t, Minion) and not t.dead:
            t.perm_atk += a
            t.perm_hp += h
    return fn


def summon_tokens(name, count):
    def fn(g, p, t):
        for _ in range(count):
            g.summon(p, get_card(name))
    return fn


def set_health(m, n):
    m.damage = 0
    m.perm_hp = 0
    m.aura_hp = 0
    m.hp_base = n


def set_attack(m, n):
    m.atk_base = n
    m.perm_atk = 0
    m.temp_atk = 0


def transform(g, m, token_name):
    board = m.owner.board
    if m not in board:
        return
    i = board.index(m)
    board.remove(m)
    nm = Minion(get_card(token_name), m.owner)
    board.insert(i, nm)
    g.recompute_auras()


def discard_random(g, p, n=1):
    for _ in range(n):
        if p.hand:
            p.hand.remove(g.rng.choice(p.hand))


# ------------------------------------------------------------------- tokens
C("The Coin", None, 0, SPELL, spell=lambda g, p, t: setattr(
    p, "mana", min(p.mana + 1, 10)), token=True)
C("Silver Hand Recruit", None, 1, MINION, 1, 1, token=True)
C("Wicked Knife", None, 1, WEAPON, 1, 0, dur=2, token=True)
C("Defender", None, 1, MINION, 2, 1, token=True)
C("Snake", None, 1, MINION, 1, 1, tribe="beast", token=True)
C("Sheep", None, 1, MINION, 1, 1, token=True)
C("Frog", None, 0, MINION, 0, 1, taunt=True, tribe="beast", token=True)
C("Hound", None, 1, MINION, 1, 1, charge=True, tribe="beast", token=True)
C("Hyena", None, 2, MINION, 2, 2, tribe="beast", token=True)
C("Spectral Spider", None, 1, MINION, 1, 1, tribe="beast", token=True)
C("Nerubian", None, 4, MINION, 4, 4, token=True)
C("Damaged Golem", None, 1, MINION, 2, 1, token=True)
C("Slime", None, 2, MINION, 2, 2, taunt=True, token=True)
C("Baine Bloodhoof", None, 4, MINION, 4, 5, token=True)
C("Whelp", None, 1, MINION, 1, 1, token=True)
C("Felwing", None, 1, MINION, 1, 1, token=True)
C("Illidari Satyr", None, 2, MINION, 2, 2, token=True)
C("Illidari Initiate", None, 1, MINION, 1, 1, rush=True, token=True)
C("Violet Apprentice", None, 1, MINION, 1, 1, token=True)
C("Treant (charge)", None, 2, MINION, 2, 2, charge=True, token=True)
C("Spirit Wolf", None, 2, MINION, 2, 3, taunt=True, token=True)
C("Ashbringer", None, 5, WEAPON, 5, 0, dur=3, token=True)
C("Light's Justice", None, 1, WEAPON, 1, 0, dur=4, token=True)
C("Second Slice", "Demon Hunter", 0, SPELL, token=True,
  spell=lambda g, p, t: setattr(p, "temp_atk", p.temp_atk + 1))


def _boombot_dr(g, p, m):
    t = rand_enemy_char(g, p)
    if t is not None:
        g.deal_damage(p, t, g.rng.randint(1, 4))


C("Boom Bot", None, 1, MINION, 1, 1, tribe="mech", deathrattle=_boombot_dr,
  token=True)

TOTEMS = ["Searing Totem", "Stoneclaw Totem", "Healing Totem",
          "Wrath of Air Totem"]
C("Searing Totem", None, 1, MINION, 1, 1, tribe="totem", token=True)
C("Stoneclaw Totem", None, 1, MINION, 0, 2, taunt=True, tribe="totem",
  token=True)
C("Wrath of Air Totem", None, 1, MINION, 0, 2, spell_dmg=1, tribe="totem",
  token=True)


def _healing_totem(g, p, m, turn_player):
    if turn_player is p:
        for fm in p.board:
            g.heal(None, fm, 1)


C("Healing Totem", None, 1, MINION, 0, 2, tribe="totem", token=True,
  triggers={"turn_end": _healing_totem})

# ---------------------------------------------------------------- neutrals
C("Abusive Sergeant", None, 1, MINION, 1, 1,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                setattr(t, "temp_atk", t.temp_atk + 2)),
  battlecry_target="minion", ai_hint=("temp_atk", 2))
C("Argent Squire", None, 1, MINION, 1, 1, divine_shield=True)
C("Leper Gnome", None, 1, MINION, 1, 1,
  deathrattle=lambda g, p, m: g.deal_damage(p, p.opponent, 2))
C("Zombie Chow", None, 1, MINION, 2, 3,
  deathrattle=lambda g, p, m: g.heal(None, p.opponent, 5))
C("Loot Hoarder", None, 2, MINION, 2, 1,
  deathrattle=lambda g, p, m: g.draw(p))


def _juggler(g, owner, m, summoning_p, summoned):
    if summoning_p is owner and summoned is not m:
        t = rand_enemy_char(g, owner)
        if t is not None:
            g.deal_damage(owner, t, 1)


C("Knife Juggler", None, 2, MINION, 3, 2, triggers={"summon": _juggler})


def _dire_wolf_aura(g, p, m):
    for n in g.adjacent(m):
        n.aura_atk += 1


C("Dire Wolf Alpha", None, 2, MINION, 2, 2, tribe="beast",
  aura=_dire_wolf_aura)
C("Bloodmage Thalnos", None, 2, MINION, 1, 1, spell_dmg=1,
  deathrattle=lambda g, p, m: g.draw(p))
C("Haunted Creeper", None, 2, MINION, 1, 2, tribe="beast",
  deathrattle=lambda g, p, m: [g.summon(p, get_card("Spectral Spider"))
                               for _ in range(2)])
C("Nerubian Egg", None, 2, MINION, 0, 2, cant_attack=True,
  deathrattle=lambda g, p, m: g.summon(p, get_card("Nerubian")))


def _mad_scientist_dr(g, p, m):
    secrets_in_deck = [c for c in p.deck if c.secret and
                       c.secret not in p.secrets]
    if secrets_in_deck and len(p.secrets) < 5:
        c = g.rng.choice(secrets_in_deck)
        p.deck.remove(c)
        p.secrets.append(c.secret)


C("Mad Scientist", None, 2, MINION, 2, 2, deathrattle=_mad_scientist_dr)
C("Ancient Watcher", None, 2, MINION, 4, 5, cant_attack=True)


def _sunfury_bc(g, p, m, t):
    for n in g.adjacent(m):
        n.taunt = True


C("Sunfury Protector", None, 2, MINION, 2, 3, battlecry=_sunfury_bc)
C("Amani Berserker", None, 2, MINION, 2, 3, enrage_atk=3)
C("Ironbeak Owl", None, 3, MINION, 2, 1,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                t.silence()),
  battlecry_target="minion", ai_hint=("silence",))
C("Harvest Golem", None, 3, MINION, 2, 3, tribe="mech",
  deathrattle=lambda g, p, m: g.summon(p, get_card("Damaged Golem")))
C("Shattered Sun Cleric", None, 3, MINION, 3, 2,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                (setattr(t, "perm_atk", t.perm_atk + 1),
                                 setattr(t, "perm_hp", t.perm_hp + 1))),
  battlecry_target="friendly_minion", ai_hint=("buff", 1, 1))
C("Earthen Ring Farseer", None, 3, MINION, 3, 3,
  battlecry=lambda g, p, m, t: g.heal(p, t if t is not None else p, 3),
  battlecry_target="any", ai_hint=("heal", 3))


def _acolyte(g, owner, m, dmg_owner, dmg_minion, amount, source):
    if dmg_minion is m:
        g.draw(owner)


C("Acolyte of Pain", None, 3, MINION, 1, 3,
  triggers={"minion_damaged": _acolyte})
C("Big Game Hunter", None, 3, MINION, 4, 2,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                t.attack >= 7 and g.destroy(t)),
  battlecry_target="enemy_minion_atk7", ai_hint=("destroy_big",))
C("Wolfrider", None, 3, MINION, 3, 1, charge=True)
C("Dark Iron Dwarf", None, 4, MINION, 4, 4,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                setattr(t, "temp_atk", t.temp_atk + 2)),
  battlecry_target="minion", ai_hint=("temp_atk", 2))


def _argus_bc(g, p, m, t):
    for n in g.adjacent(m):
        n.perm_atk += 1
        n.perm_hp += 1
        n.taunt = True


C("Defender of Argus", None, 4, MINION, 2, 3, battlecry=_argus_bc)
C("Chillwind Yeti", None, 4, MINION, 4, 5)

SHREDDER_POOL = ["Loot Hoarder", "Knife Juggler", "Dire Wolf Alpha",
                 "Haunted Creeper", "Nerubian Egg", "Mad Scientist",
                 "Ancient Watcher", "Sunfury Protector", "Amani Berserker",
                 "Bloodmage Thalnos", "Shielded Minibot", "Wild Pyromancer",
                 "Sorcerer's Apprentice"]
C("Piloted Shredder", None, 4, MINION, 4, 3, tribe="mech",
  deathrattle=lambda g, p, m: g.summon(
      p, get_card(g.rng.choice(SHREDDER_POOL))))
C("Sen'jin Shieldmasta", None, 4, MINION, 3, 5, taunt=True)
C("Twilight Drake", None, 4, MINION, 4, 1, tribe="dragon",
  battlecry=lambda g, p, m, t: setattr(m, "perm_hp",
                                       m.perm_hp + len(p.hand)))
C("Mountain Giant", None, 12, MINION, 8, 8)
C("Sea Giant", None, 10, MINION, 8, 8)
C("Antique Healbot", None, 5, MINION, 3, 3, tribe="mech",
  battlecry=lambda g, p, m, t: g.heal(None, p, 8))
C("Azure Drake", None, 5, MINION, 4, 4, spell_dmg=1, tribe="dragon",
  battlecry=lambda g, p, m, t: g.draw(p))
C("Sludge Belcher", None, 5, MINION, 3, 6, taunt=True,
  deathrattle=lambda g, p, m: g.summon(p, get_card("Slime")))


def _faceless_bc(g, p, m, t):
    if not isinstance(t, Minion) or t.dead:
        return
    m.atk_base = t.atk_base
    m.hp_base = t.hp_base
    m.perm_atk, m.perm_hp = t.perm_atk, t.perm_hp
    m.damage = t.damage
    m.taunt, m.divine_shield = t.taunt, t.divine_shield
    m.windfury, m.charge, m.rush = t.windfury, t.charge, t.rush
    m.lifesteal, m.poisonous = t.lifesteal, t.poisonous
    m.cant_attack, m.spell_dmg = t.cant_attack, t.spell_dmg
    m.deathrattles = list(t.deathrattles)
    m.triggers = dict(t.triggers)
    m.aura = t.aura
    m.enrage_atk = t.enrage_atk
    m.card = t.card
    g.recompute_auras()


C("Faceless Manipulator", None, 5, MINION, 3, 3, battlecry=_faceless_bc,
  battlecry_target="minion", ai_hint=("copy_big",))


def _kodo_bc(g, p, m, t):
    targets = [x for x in p.opponent.board if x.attack <= 2 and not x.dead]
    if targets:
        g.destroy(g.rng.choice(targets))


C("Stampeding Kodo", None, 5, MINION, 3, 5, tribe="beast", battlecry=_kodo_bc)
C("Argent Commander", None, 6, MINION, 4, 2, charge=True, divine_shield=True)
C("Leeroy Jenkins", None, 5, MINION, 6, 2, charge=True,
  battlecry=lambda g, p, m, t: [g.summon(p.opponent, get_card("Whelp"))
                                for _ in range(2)])


def _pyro(g, owner, m, caster, card):
    if caster is owner:
        for x in list(owner.board) + list(owner.opponent.board):
            g.deal_damage(owner, x, 1)


C("Wild Pyromancer", None, 2, MINION, 3, 2, triggers={"spell_cast": _pyro})


def _auctioneer(g, owner, m, caster, card):
    if caster is owner:
        g.draw(owner)


C("Gadgetzan Auctioneer", None, 5, MINION, 4, 4,
  triggers={"spell_cast": _auctioneer})


def _teacher(g, owner, m, caster, card):
    if caster is owner:
        g.summon(owner, get_card("Violet Apprentice"))


C("Violet Teacher", None, 4, MINION, 3, 5, triggers={"spell_cast": _teacher})


def _stormwind_aura(g, p, m):
    for x in p.board:
        if x is not m:
            x.aura_atk += 1
            x.aura_hp += 1


C("Stormwind Champion", None, 7, MINION, 6, 6, aura=_stormwind_aura)
C("Boulderfist Ogre", None, 6, MINION, 6, 7)


def _sylvanas_dr(g, p, m):
    opp = p.opponent
    if opp.board and len(p.board) < MAX_BOARD:
        stolen = g.rng.choice(opp.board)
        opp.board.remove(stolen)
        stolen.owner = p
        p.board.append(stolen)
        g.recompute_auras()


C("Sylvanas Windrunner", None, 6, MINION, 5, 5, deathrattle=_sylvanas_dr)
C("Cairne Bloodhoof", None, 6, MINION, 4, 5,
  deathrattle=lambda g, p, m: g.summon(p, get_card("Baine Bloodhoof")))


def _rag(g, owner, m, turn_player):
    if turn_player is owner:
        t = rand_enemy_char(g, owner)
        if t is not None:
            g.deal_damage(owner, t, 8)


C("Ragnaros the Firelord", None, 8, MINION, 8, 8, cant_attack=True,
  triggers={"turn_end": _rag})


def _alex_bc(g, p, m, t):
    tgt = t if t is not None else p.opponent
    if isinstance(tgt, Minion):
        return
    tgt.hp = 15


C("Alexstrasza", None, 9, MINION, 8, 8, tribe="dragon", battlecry=_alex_bc,
  battlecry_target="hero", ai_hint=("alexstrasza",))
C("Dr. Boom", None, 7, MINION, 7, 7,
  battlecry=lambda g, p, m, t: [g.summon(p, get_card("Boom Bot"))
                                for _ in range(2)])

# -------------------------------------------------------------------- mage
def _mana_wyrm(g, owner, m, caster, card):
    if caster is owner:
        m.perm_atk += 1


C("Mana Wyrm", "Mage", 1, MINION, 1, 3, triggers={"spell_cast": _mana_wyrm})


def _arcane_missiles(g, p, t):
    for _ in range(3 + p.spell_power):
        tgt = rand_enemy_char(g, p)
        if tgt is not None:
            g.deal_damage(p, tgt, 1)
        g.check_deaths()


C("Arcane Missiles", "Mage", 1, SPELL, spell=_arcane_missiles)
C("Mirror Image", "Mage", 1, SPELL, spell=lambda g, p, t: [
    g.summon(p, get_card("Mirror Image Token")) for _ in range(2)])
C("Mirror Image Token", None, 1, MINION, 0, 2, taunt=True, token=True)
def _frostbolt(g, p, t):
    tgt = t if t is not None else p.opponent
    g.spell_damage(p, tgt, 3)
    if not (isinstance(tgt, Minion) and tgt.dead):
        g.freeze(tgt)


C("Frostbolt", "Mage", 2, SPELL, target="any", ai_hint=("dmg", 3),
  spell=_frostbolt)
def _sorc_aura(g, p, m):
    pass  # spell cost discount handled in Player.effective_cost


C("Sorcerer's Apprentice", "Mage", 2, MINION, 3, 2, aura=_sorc_aura)
C("Arcane Intellect", "Mage", 3, SPELL, spell=draw_n(2))
C("Frost Nova", "Mage", 3, SPELL,
  spell=lambda g, p, t: [g.freeze(m) for m in p.opponent.board])
C("Counterspell", "Mage", 3, SPELL, secret="Counterspell")
C("Mirror Entity", "Mage", 3, SPELL, secret="Mirror Entity")
C("Ice Barrier", "Mage", 3, SPELL, secret="Ice Barrier")
C("Ice Block", "Mage", 3, SPELL, secret="Ice Block")
C("Fireball", "Mage", 4, SPELL, target="any", ai_hint=("dmg", 6),
  spell=dmg_spell(6))
C("Polymorph", "Mage", 4, SPELL, target="minion", ai_hint=("transform",),
  spell=lambda g, p, t: isinstance(t, Minion) and transform(g, t, "Sheep"))
C("Water Elemental", "Mage", 4, MINION, 3, 6)
C("Blizzard", "Mage", 6, SPELL, spell=aoe_enemy_minions(2, freeze=True))
C("Flamestrike", "Mage", 7, SPELL, spell=aoe_enemy_minions(4))


def _antonidas(g, owner, m, caster, card):
    if caster is owner:
        g.add_to_hand(owner, get_card("Fireball"))


C("Archmage Antonidas", "Mage", 7, MINION, 5, 7,
  triggers={"spell_cast": _antonidas})
C("Pyroblast", "Mage", 10, SPELL, target="any", ai_hint=("dmg", 10),
  spell=dmg_spell(10))

# ------------------------------------------------------------------ hunter
C("Hunter's Mark", "Hunter", 1, SPELL, target="minion",
  ai_hint=("set_hp1",),
  spell=lambda g, p, t: isinstance(t, Minion) and set_health(t, 1))
C("Explosive Trap", "Hunter", 2, SPELL, secret="Explosive Trap")
C("Freezing Trap", "Hunter", 2, SPELL, secret="Freezing Trap")
C("Snake Trap", "Hunter", 2, SPELL, secret="Snake Trap")


def _leokk_aura(g, p, m):
    for x in p.board:
        if x is not m:
            x.aura_atk += 1


C("Huffer", None, 3, MINION, 4, 2, charge=True, tribe="beast", token=True)
C("Leokk", None, 3, MINION, 2, 4, tribe="beast", aura=_leokk_aura,
  token=True)
C("Misha", None, 3, MINION, 4, 4, taunt=True, tribe="beast", token=True)
C("Animal Companion", "Hunter", 3, SPELL,
  spell=lambda g, p, t: g.summon(
      p, get_card(g.rng.choice(["Huffer", "Leokk", "Misha"]))))


def _kill_command(g, p, t):
    n = 5 if any(m.tribe == "beast" for m in p.board) else 3
    g.spell_damage(p, t if t is not None else p.opponent, n)


C("Kill Command", "Hunter", 3, SPELL, target="any", ai_hint=("dmg", 3),
  spell=_kill_command)
C("Unleash the Hounds", "Hunter", 3, SPELL,
  spell=lambda g, p, t: [g.summon(p, get_card("Hound"))
                         for _ in range(len(p.opponent.board))])
C("Eaglehorn Bow", "Hunter", 3, WEAPON, 3, 0, dur=2)


def _houndmaster_bc(g, p, m, t):
    if isinstance(t, Minion) and not t.dead and t.tribe == "beast":
        t.perm_atk += 2
        t.perm_hp += 2
        t.taunt = True


C("Houndmaster", "Hunter", 4, MINION, 4, 3, battlecry=_houndmaster_bc,
  battlecry_target="friendly_beast", ai_hint=("buff", 2, 2))
C("Savannah Highmane", "Hunter", 6, MINION, 6, 5, tribe="beast",
  deathrattle=lambda g, p, m: [g.summon(p, get_card("Hyena"))
                               for _ in range(2)])

BEAST_POOL = ["Hound", "Hyena", "Huffer", "Leokk", "Misha", "Snake",
              "Haunted Creeper", "Dire Wolf Alpha", "Savannah Highmane",
              "Stampeding Kodo"]
C("Webspinner", "Hunter", 1, MINION, 1, 1, tribe="beast",
  deathrattle=lambda g, p, m: g.add_to_hand(
      p, get_card(g.rng.choice(BEAST_POOL))))

# ----------------------------------------------------------------- warrior
C("Execute", "Warrior", 1, SPELL, target="damaged_enemy_minion",
  ai_hint=("destroy_damaged",),
  spell=lambda g, p, t: (isinstance(t, Minion) and t.damage > 0
                         and g.destroy(t)))
C("Whirlwind", "Warrior", 1, SPELL,
  spell=lambda g, p, t: [g.deal_damage(p, m, 1 + p.spell_power)
                         for m in list(p.board) + list(p.opponent.board)])
C("Shield Slam", "Warrior", 1, SPELL, target="minion",
  ai_hint=("shield_slam",),
  spell=lambda g, p, t: isinstance(t, Minion) and
  g.spell_damage(p, t, p.armor))
C("Fiery War Axe", "Warrior", 2, WEAPON, 3, 0, dur=2)


def _slam(g, p, t):
    if not isinstance(t, Minion):
        return
    g.spell_damage(p, t, 2)
    if not t.dead:
        g.draw(p)


C("Slam", "Warrior", 2, SPELL, target="minion", ai_hint=("dmg", 2),
  spell=_slam)


def _taskmaster_bc(g, p, m, t):
    if isinstance(t, Minion) and not t.dead:
        g.deal_damage(p, t, 1)
        if not t.dead:
            t.perm_atk += 2


C("Cruel Taskmaster", "Warrior", 2, MINION, 2, 2, battlecry=_taskmaster_bc,
  battlecry_target="minion", ai_hint=("taskmaster",))


def _armorsmith(g, owner, m, dmg_owner, dmg_minion, amount, source):
    if dmg_owner is owner:
        owner.armor += 1


C("Armorsmith", "Warrior", 2, MINION, 1, 4,
  triggers={"minion_damaged": _armorsmith})


def _frothing(g, owner, m, dmg_owner, dmg_minion, amount, source):
    m.perm_atk += 1


C("Frothing Berserker", "Warrior", 3, MINION, 2, 4,
  triggers={"minion_damaged": _frothing})
C("Shield Block", "Warrior", 3, SPELL,
  spell=lambda g, p, t: (g.gain_armor(p, 5), g.draw(p)))
C("Kor'kron Elite", "Warrior", 4, MINION, 4, 3, charge=True)
C("Death's Bite", "Warrior", 4, WEAPON, 4, 0, dur=2,
  deathrattle=lambda g, p, m: [g.deal_damage(p, x, 1) for x in
                               list(p.board) + list(p.opponent.board)])


def _brawl(g, p, t):
    everyone = [m for m in p.board + p.opponent.board if not m.dead]
    if len(everyone) < 2:
        return
    survivor = g.rng.choice(everyone)
    for m in everyone:
        if m is not survivor:
            g.destroy(m)


C("Brawl", "Warrior", 5, SPELL, spell=_brawl,
  play_if=lambda g, p: len(p.board) + len(p.opponent.board) >= 2)
C("Arcanite Reaper", "Warrior", 5, WEAPON, 5, 0, dur=2)
C("Shieldmaiden", "Warrior", 5, MINION, 5, 5,
  battlecry=lambda g, p, m, t: g.gain_armor(p, 5))
C("Grommash Hellscream", "Warrior", 8, MINION, 4, 9, charge=True,
  enrage_atk=6)

# ----------------------------------------------------------------- paladin
C("Blessing of Might", "Paladin", 1, SPELL, target="friendly_minion",
  ai_hint=("buff", 3, 0), spell=buff_spell(3, 0))
C("Noble Sacrifice", "Paladin", 1, SPELL, secret="Noble Sacrifice")
C("Argent Protector", "Paladin", 2, MINION, 2, 2,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                setattr(t, "divine_shield", True)),
  battlecry_target="friendly_minion", ai_hint=("divine_shield",))
C("Equality", "Paladin", 2, SPELL,
  spell=lambda g, p, t: [set_health(m, 1)
                         for m in p.board + p.opponent.board])
C("Shielded Minibot", "Paladin", 2, MINION, 2, 2, divine_shield=True,
  tribe="mech")


def _muster(g, p, t):
    for _ in range(3):
        g.summon(p, get_card("Silver Hand Recruit"))
    p.weapon = Weapon(get_card("Light's Justice"))


C("Muster for Battle", "Paladin", 3, SPELL, spell=_muster)
C("Aldor Peacekeeper", "Paladin", 3, MINION, 3, 3,
  battlecry=lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                                set_attack(t, 1)),
  battlecry_target="enemy_minion", ai_hint=("set_atk1",))
C("Truesilver Champion", "Paladin", 4, WEAPON, 4, 0, dur=2,
  triggers={"hero_attack": lambda g, p: g.heal(None, p, 2)})
C("Blessing of Kings", "Paladin", 4, SPELL, target="friendly_minion",
  ai_hint=("buff", 4, 4), spell=buff_spell(4, 4))
C("Consecration", "Paladin", 4, SPELL,
  spell=lambda g, p, t: ([g.deal_damage(p, m, 2 + p.spell_power)
                          for m in list(p.opponent.board)],
                         g.deal_damage(p, p.opponent, 2 + p.spell_power)))
C("Hammer of Wrath", "Paladin", 4, SPELL, target="any", ai_hint=("dmg", 3),
  spell=lambda g, p, t: (g.spell_damage(p, t if t is not None else
                                        p.opponent, 3), g.draw(p)))


def _quartermaster_bc(g, p, m, t):
    for x in p.board:
        if x.card.name == "Silver Hand Recruit":
            x.perm_atk += 2
            x.perm_hp += 2


C("Quartermaster", "Paladin", 5, MINION, 2, 5, battlecry=_quartermaster_bc)
C("Lay on Hands", "Paladin", 8, SPELL,
  spell=lambda g, p, t: (g.heal(None, p, 8), g.draw(p, 3)))
C("Tirion Fordring", "Paladin", 8, MINION, 6, 6, taunt=True,
  divine_shield=True,
  deathrattle=lambda g, p, m: setattr(p, "weapon",
                                      Weapon(get_card("Ashbringer"))))

# ------------------------------------------------------------------ priest
C("Circle of Healing", "Priest", 0, SPELL,
  spell=lambda g, p, t: [g.heal(p, m, 4)
                         for m in list(p.board) + list(p.opponent.board)])


def _northshire(g, owner, m, target, healed):
    g.draw(owner)


C("Northshire Cleric", "Priest", 1, MINION, 1, 3,
  triggers={"healed": _northshire})
C("Power Word: Shield", "Priest", 1, SPELL, target="minion",
  ai_hint=("buff", 0, 2),
  spell=lambda g, p, t: (buff_spell(0, 2)(g, p, t), g.draw(p)))
C("Shadow Word: Pain", "Priest", 2, SPELL, target="enemy_minion_atk_le3",
  ai_hint=("destroy_small",),
  spell=lambda g, p, t: (isinstance(t, Minion) and t.attack <= 3 and
                         g.destroy(t)))
C("Injured Blademaster", "Priest", 3, MINION, 4, 7,
  battlecry=lambda g, p, m, t: g.deal_damage(p, m, 4))
C("Shadow Word: Death", "Priest", 3, SPELL, target="enemy_minion_atk5",
  ai_hint=("destroy_big5",),
  spell=lambda g, p, t: (isinstance(t, Minion) and t.attack >= 5 and
                         g.destroy(t)))
C("Auchenai Soulpriest", "Priest", 4, MINION, 3, 5)
C("Holy Nova", "Priest", 5, SPELL,
  spell=lambda g, p, t: ([g.deal_damage(p, m, 2 + p.spell_power)
                          for m in list(p.opponent.board)],
                         g.deal_damage(p, p.opponent, 2 + p.spell_power),
                         [g.heal(None, m, 2) for m in p.board],
                         g.heal(None, p, 2)))
C("Holy Fire", "Priest", 6, SPELL, target="any", ai_hint=("dmg", 5),
  spell=lambda g, p, t: (g.spell_damage(p, t if t is not None else
                                        p.opponent, 5), g.heal(None, p, 5)))


def _mind_control(g, p, t):
    if not isinstance(t, Minion) or t.dead:
        return
    opp = t.owner
    if t in opp.board and len(p.board) < MAX_BOARD:
        opp.board.remove(t)
        t.owner = p
        t.just_summoned = True
        t.attacks_done = 0
        p.board.append(t)
        g.recompute_auras()


C("Mind Control", "Priest", 10, SPELL, target="enemy_minion",
  ai_hint=("mind_control",), spell=_mind_control)

# ------------------------------------------------------------------- rogue
C("Backstab", "Rogue", 0, SPELL, target="undamaged_minion",
  ai_hint=("dmg", 2),
  spell=lambda g, p, t: (isinstance(t, Minion) and t.damage == 0 and
                         g.spell_damage(p, t, 2)))
C("Preparation", "Rogue", 0, SPELL,
  spell=lambda g, p, t: setattr(p, "spell_discount", 3))
C("Deadly Poison", "Rogue", 1, SPELL,
  play_if=lambda g, p: p.weapon is not None,
  spell=lambda g, p, t: p.weapon and setattr(p.weapon, "atk",
                                             p.weapon.atk + 2))
C("Cold Blood", "Rogue", 1, SPELL, target="friendly_minion",
  ai_hint=("buff", 2, 0), spell=buff_spell(2, 0),
  combo_spell=buff_spell(4, 0))


def _blade_flurry(g, p, t):
    if not p.weapon:
        return
    dmg = p.weapon.atk + p.spell_power
    p.weapon = None
    for m in list(p.opponent.board):
        g.deal_damage(p, m, dmg)
    g.deal_damage(p, p.opponent, dmg)


C("Blade Flurry", "Rogue", 2, SPELL, spell=_blade_flurry,
  play_if=lambda g, p: p.weapon is not None)
C("Eviscerate", "Rogue", 2, SPELL, target="any", ai_hint=("dmg", 2),
  spell=dmg_spell(2), combo_spell=dmg_spell(4))


def _sap(g, p, t):
    if not isinstance(t, Minion) or t.dead:
        return
    owner = t.owner
    if t in owner.board:
        owner.board.remove(t)
        if len(owner.hand) < 10:
            owner.hand.append(t.card)
        g.recompute_auras()


C("Sap", "Rogue", 2, SPELL, target="enemy_minion", ai_hint=("sap",),
  spell=_sap)
C("Shiv", "Rogue", 2, SPELL, target="any", ai_hint=("dmg", 1),
  spell=lambda g, p, t: (g.spell_damage(p, t if t is not None else
                                        p.opponent, 1), g.draw(p)))
C("Fan of Knives", "Rogue", 3, SPELL,
  spell=lambda g, p, t: ([g.deal_damage(p, m, 1 + p.spell_power)
                          for m in list(p.opponent.board)], g.draw(p)))
C("SI:7 Agent", "Rogue", 3, MINION, 3, 3,
  combo=lambda g, p, m, t: g.deal_damage(
      p, t if t is not None else p.opponent, 2),
  battlecry_target="any", ai_hint=("dmg", 2))


def _edwin_bc(g, p, m, t):
    n = max(0, p.cards_played_turn - 1)
    m.perm_atk += 2 * n
    m.perm_hp += 2 * n


C("Edwin VanCleef", "Rogue", 3, MINION, 2, 2, battlecry=_edwin_bc)
C("Assassinate", "Rogue", 5, SPELL, target="enemy_minion",
  ai_hint=("destroy",),
  spell=lambda g, p, t: isinstance(t, Minion) and g.destroy(t))

# ------------------------------------------------------------------ shaman
C("Earth Shock", "Shaman", 1, SPELL, target="minion",
  ai_hint=("silence_dmg", 1),
  spell=lambda g, p, t: (isinstance(t, Minion) and not t.dead and
                         (t.silence(), g.spell_damage(p, t, 1))))
C("Lightning Bolt", "Shaman", 1, SPELL, target="any", overload=1,
  ai_hint=("dmg", 3), spell=dmg_spell(3))


def _rockbiter(g, p, t):
    if isinstance(t, Minion):
        t.temp_atk += 3
    else:
        t = t if t is not None else p
        t.temp_atk += 3


C("Rockbiter Weapon", "Shaman", 2, SPELL, target="friendly_char",
  ai_hint=("rockbiter",), spell=_rockbiter)


def _flametongue_aura(g, p, m):
    for n in g.adjacent(m):
        n.aura_atk += 2


C("Flametongue Totem", "Shaman", 2, MINION, 0, 3, tribe="totem",
  aura=_flametongue_aura)
C("Feral Spirit", "Shaman", 3, SPELL, overload=2,
  spell=summon_tokens("Spirit Wolf", 2))
C("Hex", "Shaman", 4, SPELL, target="minion", ai_hint=("transform",),
  spell=lambda g, p, t: isinstance(t, Minion) and transform(g, t, "Frog"))
C("Lava Burst", "Shaman", 3, SPELL, target="any", overload=2,
  ai_hint=("dmg", 5), spell=dmg_spell(5))


def _lightning_storm(g, p, t):
    for m in list(p.opponent.board):
        g.deal_damage(p, m, g.rng.randint(2, 3) + p.spell_power)


C("Lightning Storm", "Shaman", 3, SPELL, overload=2, spell=_lightning_storm)


def _unbound(g, owner, m, ol_player):
    if ol_player is owner:
        m.perm_atk += 1
        m.perm_hp += 1


C("Unbound Elemental", "Shaman", 3, MINION, 2, 4,
  triggers={"overload": _unbound})


def _mana_tide(g, owner, m, turn_player):
    if turn_player is owner:
        g.draw(owner)


C("Mana Tide Totem", "Shaman", 3, MINION, 0, 3, tribe="totem",
  triggers={"turn_end": _mana_tide})
C("Doomhammer", "Shaman", 5, WEAPON, 2, 0, dur=8, windfury=True, overload=2)
C("Fire Elemental", "Shaman", 6, MINION, 6, 5,
  battlecry=lambda g, p, m, t: g.deal_damage(
      p, t if t is not None else p.opponent, 3),
  battlecry_target="any", ai_hint=("dmg", 3))
C("Al'Akir the Windlord", "Shaman", 8, MINION, 3, 5, windfury=True,
  charge=True, divine_shield=True, taunt=True)
C("Bloodlust", "Shaman", 5, SPELL,
  spell=lambda g, p, t: [setattr(m, "temp_atk", m.temp_atk + 3)
                         for m in p.board])
C("Earth Elemental", "Shaman", 5, MINION, 7, 8, taunt=True, overload=3)

# ----------------------------------------------------------------- warlock
C("Flame Imp", "Warlock", 1, MINION, 3, 2, tribe="demon",
  battlecry=lambda g, p, m, t: g.deal_damage(None, p, 3))
C("Voidwalker", "Warlock", 1, MINION, 1, 3, taunt=True, tribe="demon")
C("Soulfire", "Warlock", 1, SPELL, target="any", ai_hint=("dmg", 4),
  spell=lambda g, p, t: (g.spell_damage(p, t if t is not None else
                                        p.opponent, 4),
                         discard_random(g, p, 1)))


def _mortal_coil(g, p, t):
    if not isinstance(t, Minion):
        return
    g.spell_damage(p, t, 1)
    if t.dead:
        g.draw(p)


C("Mortal Coil", "Warlock", 1, SPELL, target="minion", ai_hint=("dmg", 1),
  spell=_mortal_coil)
C("Darkbomb", "Warlock", 2, SPELL, target="any", ai_hint=("dmg", 3),
  spell=dmg_spell(3))
C("Shadow Bolt", "Warlock", 3, SPELL, target="minion", ai_hint=("dmg", 4),
  spell=dmg_spell(4))
C("Hellfire", "Warlock", 4, SPELL,
  spell=lambda g, p, t: [g.deal_damage(p, x, 3 + p.spell_power) for x in
                         list(p.board) + list(p.opponent.board) +
                         [p, p.opponent]])
C("Doomguard", "Warlock", 5, MINION, 5, 7, charge=True, tribe="demon",
  battlecry=lambda g, p, m, t: discard_random(g, p, 2))
C("Siphon Soul", "Warlock", 6, SPELL, target="minion",
  ai_hint=("destroy",),
  spell=lambda g, p, t: (isinstance(t, Minion) and
                         (g.destroy(t), g.heal(None, p, 3))))

# ------------------------------------------------------------------- druid
C("Innervate", "Druid", 0, SPELL,
  spell=lambda g, p, t: setattr(p, "mana", min(p.mana + 2, 10)))
C("Wild Growth", "Druid", 2, SPELL,
  play_if=lambda g, p: p.crystals < 10,
  spell=lambda g, p, t: setattr(p, "crystals", min(p.crystals + 1, 10)))
C("Wrath", "Druid", 2, SPELL, target="minion", ai_hint=("dmg", 3),
  choose=(lambda g, p, t: isinstance(t, Minion) and g.spell_damage(p, t, 3),
          lambda g, p, t: (isinstance(t, Minion) and g.spell_damage(p, t, 1),
                           g.draw(p))))
C("Savage Roar", "Druid", 3, SPELL,
  spell=lambda g, p, t: ([setattr(m, "temp_atk", m.temp_atk + 2)
                          for m in p.board],
                         setattr(p, "temp_atk", p.temp_atk + 2)))


def _swipe(g, p, t):
    main = t if t is not None else p.opponent
    g.spell_damage(p, main, 4)
    for x in enemy_chars(p):
        if x is not main and not (isinstance(x, Minion) and x.dead):
            g.deal_damage(p, x, 1 + p.spell_power)


C("Swipe", "Druid", 4, SPELL, target="enemy", ai_hint=("dmg", 4),
  spell=_swipe)
C("Keeper of the Grove", "Druid", 4, MINION, 2, 4,
  battlecry_target="any", ai_hint=("keeper",),
  choose=(lambda g, p, m, t: t is not None and g.deal_damage(p, t, 2),
          lambda g, p, m, t: (isinstance(t, Minion) and not t.dead and
                              t.silence())))


def _droid_claw_bc_charge(g, p, m, t):
    m.charge = True


def _droid_claw_bc_taunt(g, p, m, t):
    m.taunt = True
    m.perm_hp += 2


C("Druid of the Claw", "Druid", 5, MINION, 4, 4,
  choose=(_droid_claw_bc_charge, _droid_claw_bc_taunt))


def _force_of_nature(g, p, t):
    for _ in range(3):
        m = g.summon(p, get_card("Treant (charge)"))
        if m is not None:
            m.to_die_eot = True


C("Force of Nature", "Druid", 6, SPELL, spell=_force_of_nature)
C("Nourish", "Druid", 5, SPELL,
  choose=(lambda g, p, t: setattr(p, "crystals", min(p.crystals + 2, 10)),
          lambda g, p, t: g.draw(p, 3)))
C("Ancient of Lore", "Druid", 7, MINION, 5, 5,
  choose=(lambda g, p, m, t: g.draw(p, 2),
          lambda g, p, m, t: g.heal(None, p, 5)))


def _aow_hp(g, p, m, t):
    m.perm_hp += 5
    m.taunt = True


def _aow_atk(g, p, m, t):
    m.perm_atk += 5


C("Ancient of War", "Druid", 7, MINION, 5, 5, choose=(_aow_hp, _aow_atk))

# ------------------------------------------------------------- demon hunter
C("Twin Slice", "Demon Hunter", 0, SPELL,
  spell=lambda g, p, t: (setattr(p, "temp_atk", p.temp_atk + 1),
                         g.add_to_hand(p, get_card("Second Slice"))))
C("Battlefiend", "Demon Hunter", 1, MINION, 1, 2, tribe="demon",
  triggers={"hero_attacked": lambda g, owner, m, hp:
            hp is owner and setattr(m, "perm_atk", m.perm_atk + 1)})
C("Chaos Strike", "Demon Hunter", 2, SPELL,
  spell=lambda g, p, t: (setattr(p, "temp_atk", p.temp_atk + 2), g.draw(p)))
C("Umberwing", "Demon Hunter", 2, WEAPON, 1, 0, dur=2,
  battlecry=lambda g, p, m, t: [g.summon(p, get_card("Felwing"))
                                for _ in range(2)])
C("Satyr Overseer", "Demon Hunter", 3, MINION, 4, 2, tribe="demon",
  triggers={"hero_attacked": lambda g, owner, m, hp:
            hp is owner and g.summon(owner, get_card("Illidari Satyr"))})
C("Eye Beam", "Demon Hunter", 3, SPELL, target="minion",
  outcast_discount=2, ai_hint=("dmg", 3),
  spell=lambda g, p, t: (isinstance(t, Minion) and
                         g.deal_damage(p, t, 3 + p.spell_power) and
                         g.heal(None, p, 3 + p.spell_power)))
C("Aldrachi Warblades", "Demon Hunter", 3, WEAPON, 2, 0, dur=2,
  lifesteal=True)
C("Coordinated Strike", "Demon Hunter", 3, SPELL,
  spell=summon_tokens("Illidari Initiate", 3))
C("Chaos Nova", "Demon Hunter", 5, SPELL,
  spell=lambda g, p, t: [g.deal_damage(p, m, 4 + p.spell_power) for m in
                         list(p.board) + list(p.opponent.board)])
C("Glaivebound Adept", "Demon Hunter", 5, MINION, 6, 4,
  battlecry=lambda g, p, m, t: (p.hero_attacked_turn and
                                g.deal_damage(p, t if t is not None else
                                              p.opponent, 4)),
  battlecry_target="any", ai_hint=("dmg", 4))


def _skull(g, p, t):
    n = 3
    cost_cut = 3 if g._outcast else 0
    before = len(p.hand)
    g.draw(p, n)
    if cost_cut:
        pass  # cost reduction of drawn cards is not tracked per-copy;
        # approximated by no reduction (noted simplification)


C("Skull of Gul'dan", "Demon Hunter", 6, SPELL, outcast_discount=0,
  spell=_skull)


def _priestess(g, owner, m, turn_player):
    if turn_player is owner:
        for _ in range(6):
            t = rand_enemy_char(g, owner)
            if t is not None:
                g.deal_damage(owner, t, 1)
            g.check_deaths()


C("Priestess of Fury", "Demon Hunter", 7, MINION, 6, 7, tribe="demon",
  triggers={"turn_end": _priestess})
