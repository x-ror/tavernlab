"""Auto-compiler: parses simple card texts into engine effects.

A card is only marked implemented when its ENTIRE text is understood.
Partial matches leave the card unimplemented (excluded from pools).
"""
import re

from .engine import CardDef, Minion, MINION, SPELL, WEAPON

KEYWORD_PAT = re.compile(
    r"^(Taunt|Divine Shield|Charge|Rush|Windfury|Stealth|Lifesteal|"
    r"Poisonous|Elusive|Reborn|Tradeable|Prepare|Dormant for \d+ turns?|"
    r"Spell Damage \+\d+|Overload:?\s*\(\d+\))[.,!]?\s*", re.I)

_SYNTH = {}


def synth_token(atk, hp, kws):
    key = (atk, hp, tuple(sorted(kws)))
    if key in _SYNTH:
        return _SYNTH[key]
    d = CardDef(id=f"synth_{atk}_{hp}_{'_'.join(kws)}",
                name=f"{atk}/{hp} Token", type=MINION, cls="NEUTRAL",
                cost=1, atk=atk, hp=hp, races=[], text="", coll=False,
                implemented=True,
                **{k: True for k in kws})
    _SYNTH[key] = d
    return d


KW_WORDS = {"taunt": "taunt", "rush": "rush", "charge": "charge",
            "divine shield": "divine_shield", "lifesteal": "lifesteal",
            "windfury": "windfury", "stealth": "stealth",
            "poisonous": "poisonous", "reborn": "reborn"}
NUMWORD = {"a": 1, "an": 1, "one": 1, "two": 2, "three": 3, "four": 4,
           "five": 5}


def strip_keywords(text):
    while True:
        new = KEYWORD_PAT.sub("", text)
        if new == text:
            return text.strip()
        text = new


# ---------------------------------------------------------------- effects
def fx_dmg(n, who):
    def fn(g, p, t):
        opp = p.opponent
        if who == "aoe_enemy_minions":
            for m in list(opp.active_minions):
                g.deal_damage(p, m, n + p.spell_power)
        elif who == "aoe_all_minions":
            for m in list(p.active_minions) + list(opp.active_minions):
                g.deal_damage(p, m, n + p.spell_power)
        elif who == "aoe_enemies":
            for m in list(opp.active_minions):
                g.deal_damage(p, m, n + p.spell_power)
            g.deal_damage(p, opp, n + p.spell_power)
        elif who == "aoe_all_chars":
            for m in list(p.active_minions) + list(opp.active_minions):
                g.deal_damage(p, m, n + p.spell_power)
            g.deal_damage(p, p, n + p.spell_power)
            g.deal_damage(p, opp, n + p.spell_power)
        elif who == "split_enemies":
            for _ in range(n + p.spell_power):
                opts = [opp] + list(opp.active_minions)
                opts = [x for x in opts
                        if not (isinstance(x, Minion) and x.dead)]
                if opts:
                    g.deal_damage(p, g.rng.choice(opts), 1)
                g.check_deaths()
        elif who == "split_enemy_minions":
            for _ in range(n + p.spell_power):
                opts = [x for x in opp.active_minions if not x.dead]
                if opts:
                    g.deal_damage(p, g.rng.choice(opts), 1)
                g.check_deaths()
        else:
            g.spell_damage(p, t if t is not None else opp, n)
    return fn


def fx_bc_dmg(n, aoe=None):
    def fn(g, p, m, t):
        if aoe == "enemy_minions":
            for x in list(p.opponent.active_minions):
                g.deal_damage(p, x, n)
        elif aoe == "split_enemies":
            for _ in range(n):
                opts = [p.opponent] + list(p.opponent.active_minions)
                opts = [x for x in opts
                        if not (isinstance(x, Minion) and x.dead)]
                if opts:
                    g.deal_damage(p, g.rng.choice(opts), 1)
        elif t is not None:
            g.deal_damage(p, t, n)
    return fn


def fx_draw(n):
    return lambda g, p, t=None, *a: g.draw(p, n)


def fx_heal_hero(n):
    return lambda g, p, t=None, *a: g.heal(p, p, n)


def fx_heal_target(n):
    return lambda g, p, t: g.heal(p, t if t is not None else p, n)


def fx_armor(n):
    return lambda g, p, t=None, *a: g.gain_armor(p, n)


def fx_summon(count, atk, hp, kws):
    tok = synth_token(atk, hp, kws)
    def fn(g, p, t=None, *a):
        for _ in range(count):
            g.summon(p, tok)
    return fn


def fx_buff(a, h, temp=False):
    def fn(g, p, t):
        if isinstance(t, Minion) and not t.dead:
            if temp:
                t.temp_atk += a
            else:
                t.perm_atk += a
                t.perm_hp += h
    return fn


def fx_hero_atk(n):
    return lambda g, p, t=None, *a: setattr(p, "temp_atk", p.temp_atk + n)


def fx_freeze():
    def fn(g, p, t):
        if t is not None:
            g.freeze(t)
    return fn


# ------------------------------------------------------------- the parser
SEG_PATTERNS = []


def seg(pattern):
    def deco(fn):
        SEG_PATTERNS.append((re.compile(pattern + r"$", re.I), fn))
        return fn
    return deco


def parse_kws(s):
    kws = []
    if s:
        for w, attr in KW_WORDS.items():
            if w in s.lower():
                kws.append(attr)
    return kws


@seg(r"Deal \$?(\d+) damage(?: to a (?:minion|character))?\.?")
def _p_dmg(m, card):
    n = int(m.group(1))
    tgt = "minion" if m.group(0).find("minion") >= 0 else "any"
    return {"kind": "dmg", "n": n, "target": tgt}


@seg(r"Deal \$?(\d+) damage to all enemy minions\.?")
def _p_aoe_em(m, card):
    return {"kind": "dmg_aoe", "n": int(m.group(1)),
            "who": "aoe_enemy_minions"}


@seg(r"Deal \$?(\d+) damage to all minions\.?")
def _p_aoe_all(m, card):
    return {"kind": "dmg_aoe", "n": int(m.group(1)),
            "who": "aoe_all_minions"}


@seg(r"Deal \$?(\d+) damage to all enemies\.?")
def _p_aoe_e(m, card):
    return {"kind": "dmg_aoe", "n": int(m.group(1)), "who": "aoe_enemies"}


@seg(r"Deal \$?(\d+) damage to ALL characters\.?")
def _p_aoe_c(m, card):
    return {"kind": "dmg_aoe", "n": int(m.group(1)), "who": "aoe_all_chars"}


@seg(r"Deal \$?(\d+) damage randomly split among (?:all )?enemies\.?")
def _p_split(m, card):
    return {"kind": "dmg_aoe", "n": int(m.group(1)), "who": "split_enemies"}


@seg(r"Deal \$?(\d+) damage randomly split among (?:all )?enemy minions\.?")
def _p_split_m(m, card):
    return {"kind": "dmg_aoe", "n": int(m.group(1)),
            "who": "split_enemy_minions"}


@seg(r"Draw (a|\d+) cards?\.?")
def _p_draw(m, card):
    w = m.group(1)
    return {"kind": "draw", "n": 1 if w == "a" else int(w)}


@seg(r"Restore #?(\d+) Health to your hero\.?")
def _p_healh(m, card):
    return {"kind": "heal_hero", "n": int(m.group(1))}


@seg(r"Restore #?(\d+) Health\.?")
def _p_healt(m, card):
    return {"kind": "heal", "n": int(m.group(1))}


@seg(r"Gain (\d+) Armor\.?")
def _p_armor(m, card):
    return {"kind": "armor", "n": int(m.group(1))}


@seg(r"Summon (a|an|two|three|four) (\d+)/(\d+) [A-Za-z' ]+?"
     r"(?: with ([A-Za-z ,]+?))?\.?")
def _p_summon(m, card):
    count = NUMWORD.get(m.group(1).lower(), 1)
    return {"kind": "summon", "count": count, "atk": int(m.group(2)),
            "hp": int(m.group(3)), "kws": parse_kws(m.group(4))}


@seg(r"Give a (?:friendly )?minion \+(\d+)/\+(\d+)\.?")
def _p_buff(m, card):
    return {"kind": "buff", "a": int(m.group(1)), "h": int(m.group(2)),
            "friendly": "friendly" in m.group(0)}


@seg(r"Give your hero \+(\d+) Attack this turn\.?")
def _p_hatk(m, card):
    return {"kind": "hero_atk", "n": int(m.group(1))}


@seg(r"(?:and )?Freeze (?:it|a minion|a character)\.?")
def _p_frz(m, card):
    return {"kind": "freeze"}


def split_segments(text):
    # split on sentence boundaries, keeping it simple
    parts = [s.strip() for s in re.split(r"(?<=[.!])\s+", text) if s.strip()]
    return parts


def compile_effects(text, card):
    """Returns list of effect dicts, or None if any segment fails."""
    out = []
    for segtext in split_segments(text):
        matched = None
        for pat, fn in SEG_PATTERNS:
            mm = pat.match(segtext)
            if mm:
                matched = fn(mm, card)
                break
        if matched is None:
            return None
        out.append(matched)
    return out


def effects_to_spell(effs, card):
    fns = []
    target = None
    hint = ()
    for e in effs:
        k = e["kind"]
        if k == "dmg":
            fns.append(fx_dmg(e["n"], "target"))
            target = "minion" if e["target"] == "minion" else "any"
            hint = ("dmg", e["n"])
        elif k == "dmg_aoe":
            fns.append(fx_dmg(e["n"], e["who"]))
        elif k == "draw":
            fns.append(fx_draw(e["n"]))
        elif k == "heal_hero":
            fns.append(fx_heal_hero(e["n"]))
        elif k == "heal":
            fns.append(fx_heal_target(e["n"]))
            target = target or "any"
            hint = hint or ("heal", e["n"])
        elif k == "armor":
            fns.append(fx_armor(e["n"]))
        elif k == "summon":
            fns.append(fx_summon(e["count"], e["atk"], e["hp"], e["kws"]))
        elif k == "buff":
            fns.append(fx_buff(e["a"], e["h"]))
            target = "friendly_minion" if e["friendly"] else "minion"
            hint = ("buff", e["a"], e["h"])
        elif k == "hero_atk":
            fns.append(fx_hero_atk(e["n"]))
        elif k == "freeze":
            fns.append(fx_freeze())
            target = target or "any"
        else:
            return None

    def spell(g, p, t):
        for fn in fns:
            fn(g, p, t)
            if g.over:
                return
    return spell, target, hint


def effects_to_battlecry(effs, card):
    res = effects_to_spell(effs, card)
    if res is None:
        return None
    spell, target, hint = res

    def bc(g, p, m, t):
        spell(g, p, t)
    return bc, target, hint


LIFESTEAL_SPELL = re.compile(r"^Lifesteal[.,]?\s*", re.I)


def try_compile(card):
    """Attach behaviors if the whole text parses. Mutates card."""
    text = strip_keywords(card.text or "")
    if card.type == MINION:
        if text == "":
            card.implemented = True
            return True
        mbc = re.match(r"^Battlecry:\s*(.+)$", text)
        mdr = re.match(r"^Deathrattle:\s*(.+)$", text)
        if mbc:
            effs = compile_effects(mbc.group(1), card)
            if effs is None:
                return False
            res = effects_to_battlecry(effs, card)
            if res is None:
                return False
            card.battlecry, tgt, hint = res
            card.battlecry_target = tgt
            card.ai_hint = hint
            card.implemented = True
            return True
        if mdr:
            effs = compile_effects(mdr.group(1), card)
            if effs is None:
                return False
            res = effects_to_battlecry(effs, card)
            if res is None:
                return False
            bc, _, _ = res
            card.deathrattle = lambda g, p, m, _bc=bc: _bc(g, p, m, None)
            card.implemented = True
            return True
        return False
    if card.type == SPELL:
        if "Secret:" in text or "Quest" in text or "Choose One" in text:
            return False
        life = bool(LIFESTEAL_SPELL.match(text))
        text2 = LIFESTEAL_SPELL.sub("", text)
        effs = compile_effects(text2, card)
        if effs is None:
            return False
        res = effects_to_spell(effs, card)
        if res is None:
            return False
        spell, tgt, hint = res
        if life:
            base = spell
            dmg_n = next((e["n"] for e in effs if e["kind"] == "dmg"), 0)

            def spell(g, p, t, _b=base, _n=dmg_n):
                _b(g, p, t)
                g.heal(p, p, _n + p.spell_power)
        card.spell = spell
        card.target = tgt
        card.ai_hint = hint
        card.implemented = True
        return True
    if card.type == WEAPON:
        if text == "":
            card.implemented = True
            return True
        return False
    return False
