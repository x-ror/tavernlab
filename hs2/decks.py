"""Deck loading for engine v2.

One gauntlet file per format, named the way the corpora are:
`meta_decks_2026.json` is the Standard gauntlet, `wild_decks.json` the
Wild one. A format with no gauntlet yet returns an empty list rather
than pretending, so a caller can say so instead of dividing by zero.
"""
import json
import os

from . import carddata, formats

_HERE = os.path.dirname(__file__)

ARCHETYPES = {
    "UUB Egg Death Knight": "midrange",
    "Quest Demon Hunter": "midrange",
    "Attack Druid": "aggro",
    "Quest Hunter": "midrange",
    "Burn Mage": "midrange",
    "Fatigue Paladin": "control",
    "Thief Priest": "control",
    "Herald Rogue": "midrange",
    "Zee Shaman": "midrange",
    "Herald Warlock": "control",
    "Dragon Warrior": "midrange",
    "Raza Demon Hunter": "midrange",
}


class Deck:
    def __init__(self, name, cls, archetype, card_ids):
        self.name = name
        self.cls = cls
        self.archetype = archetype
        self.card_ids = list(card_ids)

    @classmethod
    def from_names(cls, name, klass, archetype, cardlist):
        ids = []
        for cn, cnt in cardlist:
            d = carddata.get_def(cn)
            if not d.implemented:
                raise ValueError(f"{name}: card not implemented: {cn}")
            for _ in range(cnt):
                ids.append(d.id)
        return cls(name, klass, archetype, ids)


GAUNTLETS = {
    formats.STANDARD: "meta_decks_2026.json",
    formats.WILD: "wild_decks.json",
}


def gauntlet_path(fmt=formats.STANDARD):
    try:
        return os.path.join(_HERE, GAUNTLETS[fmt])
    except KeyError:
        raise ValueError(f"no gauntlet for format {fmt!r}") from None


def load_meta(fmt=formats.STANDARD):
    path = gauntlet_path(fmt)
    if not os.path.exists(path):
        return []          # no gauntlet built for this format yet
    raw = json.load(open(path, encoding="utf-8"))
    out = []
    for name, info in raw.items():
        cardlist = [(cn, cnt) for cn, cnt in info["cards"]]
        # Commander Beatrix: 10 copies of the sideboard 2-cost minion
        for sb_name, _ in info.get("sideboard", []):
            if any(cn == "Commander Beatrix" for cn, _c in cardlist):
                cardlist.append((sb_name, 10))
        deck = Deck.from_names(name, info["class"],
                               ARCHETYPES.get(name, "midrange"), cardlist)
        out.append(deck)
    return out


# The eleven classic heroes. A deckstring needs a hero, and the class
# alone does not name one.
HERO_IDS = {
    "WARRIOR": "HERO_01", "SHAMAN": "HERO_02", "ROGUE": "HERO_03",
    "PALADIN": "HERO_04", "HUNTER": "HERO_05", "DRUID": "HERO_06",
    "WARLOCK": "HERO_07", "MAGE": "HERO_08", "PRIEST": "HERO_09",
    "DEMONHUNTER": "HERO_10", "DEATHKNIGHT": "HERO_11",
}


def export_gauntlet(fmt=formats.STANDARD):
    """Gauntlet decks as importable deckstrings, keyed by name.

    Encoded from the **raw** gauntlet file, not from `load_meta()`:
    that flattens a sideboard into the deck (ten copies of Beatrix's
    minion), which is right for the simulator and would be an illegal
    30-card list in the game.

    `complete` is False only when the code is missing something the
    gauntlet file *has*: a sideboard, whose cards the file records
    without the card that owns them, so it cannot be encoded.

    Deck size is not checked. The gauntlet file is the authority on what
    a deck is — Thief Priest is a 20-card build and that is the deck,
    not a truncated one.
    """
    from hs2 import carddata, deckstring
    path = gauntlet_path(fmt)
    if not os.path.exists(path):
        return {}
    carddata.ensure_defs(fmt)
    by_name = {d.name: d for d in carddata.DEFS.values()}
    wire_fmt = 1 if fmt == formats.WILD else 2

    out = {}
    for name, info in json.load(open(path, encoding="utf-8")).items():
        hero = carddata.DEFS.get(HERO_IDS.get(info["class"], ""))
        if hero is None:
            continue
        cards, missing = [], []
        for cn, cnt in info["cards"]:
            card = by_name.get(cn)
            if card is None:
                missing.append(cn)
                continue
            cards.append((card.dbf, cnt))
        if missing or not cards:
            continue
        out[name] = {
            "code": deckstring.encode(hero.dbf, cards, fmt=wire_fmt),
            "cards": sum(n for _dbf, n in cards),
            "complete": not info.get("sideboard"),
        }
    return out


def check_all_implemented(fmt=formats.STANDARD):
    path = gauntlet_path(fmt)
    if not os.path.exists(path):
        return {}
    raw = json.load(open(path, encoding="utf-8"))
    missing = {}
    for name, info in raw.items():
        for cn, cnt in info["cards"] + info.get("sideboard", []):
            d = carddata.get_def(cn)
            if not d.implemented:
                missing.setdefault(cn, []).append(name)
    return missing
