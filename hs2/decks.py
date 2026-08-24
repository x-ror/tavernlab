"""Deck loading for engine v2 (Standard 2026)."""
import json
import os

from . import carddata

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


def load_meta():
    raw = json.load(open(os.path.join(_HERE, "meta_decks_2026.json")))
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


def check_all_implemented():
    raw = json.load(open(os.path.join(_HERE, "meta_decks_2026.json")))
    missing = {}
    for name, info in raw.items():
        for cn, cnt in info["cards"] + info.get("sideboard", []):
            d = carddata.get_def(cn)
            if not d.implemented:
                missing.setdefault(cn, []).append(name)
    return missing
