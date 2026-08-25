"""Build the engine's card corpus from a merged card dump.

The dump comes from `scripts/build_cards.py`, which joins Blizzard's live
card-library API with the official CardDefs.xml. It replaced the third-party
HearthstoneJSON mirror this script used to read, so the download lives there
now — one deliberate fetch, by hand, and the app itself never talks to the
network:

    python3 scripts/build_cards.py --include-carddefs-only --out cards_merged.json

Two files, because Standard is a strict subset of Wild and duplicating
1179 entries into a second file helps nobody:

    hs2/standard_cards.json   Standard-legal sets + tokens + hero powers
    hs2/wild_cards.json       everything Wild adds on top (the delta)

`carddata` loads the first and merges the second **if it exists**, so a
checkout that never built the Wild delta behaves exactly as it did
before. Legality is not decided here — the corpus carries `set`, and
`hs2/formats.py` turns that into an answer per format.

    python3 hs2/build_data.py cards_merged.json                 # Standard
    python3 hs2/build_data.py cards_merged.json --format wild   # Wild delta
    python3 hs2/build_data.py cards_merged.json --format both

The corpus needs the tokens, hero powers and enchantments the live API does
not serve, so build the dump with --include-carddefs-only.
"""
import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hs2 import formats  # noqa: E402  (needs the path above)

_HERE = os.path.dirname(os.path.abspath(__file__))

STANDARD_PATH = os.path.join(_HERE, "standard_cards.json")
WILD_PATH = os.path.join(_HERE, "wild_cards.json")

TYPES = {"MINION", "SPELL", "WEAPON", "LOCATION", "HERO", "HERO_POWER"}
# referenced cards living outside the Standard sets (basic hero powers,
# class tokens, etc.)
WHITELIST = {"The Coin", "Frail Ghoul", "Arcane Missiles", "Wicked Knife",
             "Silver Hand Recruit", "Fireblast", "Steady Shot", "Life Tap",
             "Lesser Heal", "Armor Up!", "Reinforce", "Totemic Call",
             "Dagger Mastery", "Shapeshift", "Demon Claws", "Ghoul Charge",
             "Searing Totem", "Stoneclaw Totem", "Healing Totem",
             "Strength Totem", "Sheep", "Zombeast"}


def clean_text(t):
    t = re.sub(r"</?[bi]>|\[x\]", "", t or "")
    return re.sub(r"[ \t]+", " ", t.replace("\n", " ")).strip()


def entry(c):
    e = {
        "id": c["id"],
        "dbf": c["dbfId"],
        "name": c["name"],
        "type": c["type"],
        # `or`, not a .get default: the source may carry the key with a null
        # value for cards that have no class at all.
        "cls": c.get("cardClass") or "NEUTRAL",
        "cost": c.get("cost", 0),
        "atk": c.get("attack", 0),
        "hp": c.get("health", 0),
        "dur": c.get("durability", 0),
        "armor": c.get("armor", 0),
        # `minionType` is what scripts/build_cards.py calls it; `races` is the
        # HearthstoneJSON spelling that older dumps use.
        "races": c.get("races") or c.get("minionType") or [],
        "school": c.get("spellSchool"),
        "mech": c.get("mechanics", []),
        "text": clean_text(c.get("text")),
        "coll": bool(c.get("collectible")),
        "rarity": c.get("rarity"),
        "set": c.get("set"),
    }
    # LOCATION and WEAPON durability is stored in "health" upstream. Without
    # the weapon half of this every weapon lands in the corpus with dur 0, and
    # `engine.py` destroys a weapon the moment its durability hits zero.
    if e["type"] in ("LOCATION", "WEAPON") and not e["dur"]:
        e["dur"] = e["hp"]
    return e


def _wanted(c, sets):
    """Cards from these sets, plus the tokens and hero powers they need."""
    if c.get("type") not in TYPES:
        return False
    if c.get("set") in formats.EXCLUDED_SETS:
        # WHITELIST matches by name, so without this the retired
        # Classic printing of e.g. Arcane Missiles rides in and ends
        # up legal in no format at all.
        return False
    return (c.get("set") in sets or c.get("name") in WHITELIST
            or c.get("type") == "HERO")


def build_standard(cards):
    out = {c["id"]: entry(c) for c in cards
           if _wanted(c, formats.STANDARD_SETS)}
    return out


def build_wild_delta(cards, standard):
    """Everything Wild adds. `standard` is what the other file already has."""
    return {c["id"]: entry(c) for c in cards
            if _wanted(c, formats.WILD_SETS) and c["id"] not in standard}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("src", nargs="?", default="cards_merged.json",
                    help="merged dump from scripts/build_cards.py")
    ap.add_argument("--format", choices=("standard", "wild", "both"),
                    default="standard")
    args = ap.parse_args(argv)

    with open(args.src, encoding="utf-8") as fh:
        cards = json.load(fh)
    if isinstance(cards, dict):
        cards = cards["cards"]      # --wrap output carries meta around the array

    if args.format in ("standard", "both"):
        standard = build_standard(cards)
        json.dump(standard, open(STANDARD_PATH, "w"))
        print(f"{len(standard)} entries -> {STANDARD_PATH}")
    if args.format in ("wild", "both"):
        # The delta has to complement the Standard file that actually
        # ships, not a fresh build of it: upstream data moves, and a
        # delta computed against a newer Standard would leave holes.
        if args.format == "wild" and os.path.exists(STANDARD_PATH):
            with open(STANDARD_PATH, encoding="utf-8") as fh:
                standard = json.load(fh)
        else:
            standard = build_standard(cards)
        delta = build_wild_delta(cards, standard)
        json.dump(delta, open(WILD_PATH, "w"))
        print(f"{len(delta)} entries -> {WILD_PATH}  (Wild delta)")


if __name__ == "__main__":
    main()
