"""Load official card data, merge behavior overlays, expose lookups.

A card is playable in the simulator only if it is *fully implemented*:
either its text is completely covered by the auto-compiler (autogen) or a
hand-written behavior exists in impls.BEHAVIORS. Anything else is flagged
unimplemented and excluded from pools; decks containing such cards fail
loudly at load time.
"""
import json
import os
import re

from .engine import CardDef, CardInst, MINION, SPELL, WEAPON, LOCATION, HERO

_HERE = os.path.dirname(__file__)

RAW = json.load(open(os.path.join(_HERE, "standard_cards.json")))

DEFS = {}
BY_NAME = {}


def _keywords_from(e):
    mech = set(e["mech"])
    text = e["text"] or ""
    kw = {
        "taunt": "TAUNT" in mech,
        "divine_shield": "DIVINE_SHIELD" in mech,
        "charge": "CHARGE" in mech,
        "rush": "RUSH" in mech,
        "windfury": "WINDFURY" in mech,
        "stealth": "STEALTH" in mech,
        "lifesteal": "LIFESTEAL" in mech,
        "poisonous": "POISONOUS" in mech,
        "elusive": "ELUSIVE" in mech,
        "reborn": "REBORN" in mech,
        "cant_attack": "CANT_ATTACK" in mech,
        "tradeable": "TRADEABLE" in mech,
    }
    m = re.search(r"Spell Damage \+(\d+)", text)
    kw["spell_dmg"] = int(m.group(1)) if m else 0
    m = re.search(r"Dormant for (\d+) turn", text)
    kw["dormant"] = int(m.group(1)) if m else 0
    kw["prepare"] = bool(re.match(r"Prepare[.,\s]", text)) or \
        text.startswith("Prepare")
    m = re.search(r"Overload:?\s*\((\d+)\)", text)
    kw["overload"] = int(m.group(1)) if m else (
        1 if "OVERLOAD" in mech else 0)
    return kw


def build_defs():
    from . import impls
    from . import autogen
    for cid, e in RAW.items():
        kw = _keywords_from(e)
        beh = impls.BEHAVIORS.get(cid)
        if beh is None:
            beh = impls.BEHAVIORS.get(e["name"])
        registered = beh is not None
        beh = beh or {}
        d = CardDef(
            id=cid, dbf=e["dbf"], name=e["name"], type=e["type"],
            cls=e["cls"], cost=e["cost"], atk=e["atk"], hp=e["hp"],
            dur=e["dur"], armor=e["armor"], races=e["races"],
            school=e["school"], text=e["text"], coll=e["coll"],
            rarity=e["rarity"], set=e["set"], **kw)
        for k, v in beh.items():
            setattr(d, k, v)
        if registered:
            d.implemented = True
        DEFS[cid] = d
    # autogen pass for cards without hand-written behavior
    for d in DEFS.values():
        if not d.implemented:
            autogen.try_compile(d)
    # name index: prefer collectible, then CORE set, then anything
    for cid, d in DEFS.items():
        cur = BY_NAME.get(d.name)
        if cur is None:
            BY_NAME[d.name] = cid
        else:
            c = DEFS[cur]
            if (d.coll, d.set == "CORE") > (c.coll, c.set == "CORE"):
                BY_NAME[d.name] = cid
    impls.post_build(DEFS, BY_NAME)


def get_def(name_or_id):
    if name_or_id in DEFS:
        return DEFS[name_or_id]
    cid = BY_NAME.get(name_or_id)
    if cid is None:
        raise KeyError(f"unknown card: {name_or_id!r}")
    return DEFS[cid]


def make_inst(cid):
    return CardInst(DEFS[cid])


def make_inst_by_name(name):
    return CardInst(get_def(name))


def make_inst_any(name_or_id):
    return CardInst(get_def(name_or_id))


HERO_POWER_IDS = {
    "MAGE": "Fireblast", "HUNTER": "Steady Shot", "WARLOCK": "Life Tap",
    "PRIEST": "Lesser Heal", "WARRIOR": "Armor Up!", "PALADIN": "Reinforce",
    "SHAMAN": "Totemic Call", "ROGUE": "Dagger Mastery",
    "DRUID": "Shapeshift", "DEMONHUNTER": "Demon Claws",
    "DEATHKNIGHT": "Ghoul Charge",
}


def hero_power_for(cls):
    return get_def(HERO_POWER_IDS[cls])


def standard_pool(filt=None):
    """Implemented, collectible Standard cards (for pools/discover)."""
    out = [d for d in DEFS.values()
           if d.coll and d.implemented and d.type != HERO]
    if filt:
        out = [d for d in out if filt(d)]
    return out


def coverage_report():
    coll = [d for d in DEFS.values() if d.coll]
    impl = [d for d in coll if d.implemented]
    return len(impl), len(coll)
