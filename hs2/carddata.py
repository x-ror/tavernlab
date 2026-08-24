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

from . import formats

from .engine import CardDef, CardInst, MINION, SPELL, WEAPON, LOCATION, HERO

_HERE = os.path.dirname(__file__)

def _load_corpus(fmt):
    """The card data for one format.

    Standard is the file that has always been here. Wild adds
    `wild_cards.json` on top, and that file is optional: a checkout
    that never ran `build_data.py --format wild` simply has no Wild.

    This is per format and not a union, because the union changes
    Standard play: `impls.post_build` synthesises tokens only when the
    name is absent, and Wild prints several of them; discover pools
    would grow as well. A Standard game must see the Standard corpus.
    """
    with open(os.path.join(_HERE, "standard_cards.json"),
              encoding="utf-8") as fh:
        raw = json.load(fh)
    if fmt == formats.WILD:
        wild = os.path.join(_HERE, "wild_cards.json")
        if os.path.exists(wild):
            with open(wild, encoding="utf-8") as fh:
                raw.update(json.load(fh))
    # Retired formats are dropped here and not only at build time: the
    # shipped `standard_cards.json` predates that filter and still carries
    # the Classic printing of Arcane Missiles, which is legal nowhere and
    # would make any deck holding it unresolvable.
    return {cid: e for cid, e in raw.items()
            if e.get("set") not in formats.EXCLUDED_SETS}


_WILD_INDEX = None


def wild_name_for(dbf):
    """Name of a Wild-only card, without building its whole corpus.

    A Standard deck full of Wild cards should say *which* cards are Wild,
    not print a list of unresolved dbf ids. Reads the delta file directly;
    returns None when it was never built.
    """
    global _WILD_INDEX
    if _WILD_INDEX is None:
        path = os.path.join(_HERE, "wild_cards.json")
        try:
            with open(path, encoding="utf-8") as fh:
                _WILD_INDEX = {e["dbf"]: e["name"]
                               for e in json.load(fh).values()}
        except (OSError, ValueError):
            _WILD_INDEX = {}
    return _WILD_INDEX.get(dbf)


RAW = {}
DEFS = {}
BY_NAME = {}
#: which format `DEFS` currently holds, or None before the first build
LOADED_FORMAT = None


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


def build_defs(fmt=formats.STANDARD):
    global RAW, LOADED_FORMAT
    from . import impls
    from . import autogen
    RAW = _load_corpus(fmt)
    DEFS.clear()
    BY_NAME.clear()
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
    # Name index: which printing a deck list resolves to when a name
    # appears more than once. Collectible first, then the tiers below.
    #
    # A collectible card and a hero portrait can share a name: "Irida
    # Sinseeker" is both a 4-mana minion and a 0-cost HERO_SKINS portrait
    # with no text. Without this tier the portrait can win, and because
    # behaviours are also matched by name the portrait then carries the
    # real card's code while keeping the portrait's blank stats. Three
    # gauntlet decks were built with such blanks.
    #
    # Scoped to portraits and not to the whole set: HERO_SKINS also
    # catalogues the basic hero powers (Lesser Heal, Life Tap, Armor Up!),
    # which the engine resolves by name and must keep.
    def not_a_portrait(d):
        return not (d.set == "HERO_SKINS" and d.type == "HERO")

    if fmt == formats.WILD:
        def rank(d):
            # Prefer a legal printing, then a Standard-legal one, so a
            # Standard deck never resolves to a Wild-only reprint.
            return (d.coll, formats.is_legal(d, fmt),
                    d.set in formats.STANDARD_SETS, d.set == "CORE")
    else:
        def rank(d):
            return (d.coll, not_a_portrait(d), d.set == "CORE")

    for cid, d in DEFS.items():
        cur = BY_NAME.get(d.name)
        if cur is None or rank(d) > rank(DEFS[cur]):
            BY_NAME[d.name] = cid
    impls.post_build(DEFS, BY_NAME)
    LOADED_FORMAT = fmt


def ensure_defs(fmt=formats.STANDARD):
    """Idempotent `build_defs`, for anyone who cannot know if it ran.

    Windows has no fork, so `multiprocessing` starts pool workers from a
    fresh interpreter: the defs the parent built are not inherited, and
    every worker has to build its own before it can look a card up.

    Rebuilds if a different format is loaded, since the corpus - and
    therefore every discover pool - differs between them.
    """
    if LOADED_FORMAT != fmt:
        build_defs(fmt)


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
    """Implemented, collectible cards of the loaded format.

    Named for Standard because that is what it was; the pool now
    follows `LOADED_FORMAT`, so a Wild game discovers Wild cards.
    """
    out = [d for d in DEFS.values()
           if d.coll and d.implemented and d.type != HERO]
    if filt:
        out = [d for d in out if filt(d)]
    return out


def coverage_report():
    coll = [d for d in DEFS.values() if d.coll]
    impl = [d for d in coll if d.implemented]
    return len(impl), len(coll)
