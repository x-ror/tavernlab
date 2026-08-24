"""Build standard_cards.json from HearthstoneJSON cards.json.

Includes every card from Standard-legal sets (collectible + tokens/hero
powers), keeping only the fields the engine needs. Run:
    python3 hs2/build_data.py path/to/cards.json
"""
import json
import re
import sys

STD_SETS = {"CORE", "EMERALD_DREAM", "THE_LOST_CITY", "TIME_TRAVEL",
            "CATACLYSM", "ESCAPEFROM_VIOLET_HOLD", "EVENT", "PATH_OF_ARTHAS"}
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


def build(src, dst):
    cards = json.load(open(src))
    out = {}
    for c in cards:
        listed = (c.get("set") in STD_SETS or c.get("name") in WHITELIST
                  or c.get("type") == "HERO")
        if not listed or c.get("type") not in TYPES:
            continue
        e = {
            "id": c["id"],
            "dbf": c["dbfId"],
            "name": c["name"],
            "type": c["type"],
            "cls": c.get("cardClass", "NEUTRAL"),
            "cost": c.get("cost", 0),
            "atk": c.get("attack", 0),
            "hp": c.get("health", 0),
            "dur": c.get("durability", 0),
            "armor": c.get("armor", 0),
            "races": c.get("races", []),
            "school": c.get("spellSchool"),
            "mech": c.get("mechanics", []),
            "text": clean_text(c.get("text")),
            "coll": bool(c.get("collectible")),
            "rarity": c.get("rarity"),
            "set": c.get("set"),
        }
        # LOCATION durability is stored in "health" by HSJSON
        if e["type"] == "LOCATION" and not e["dur"]:
            e["dur"] = e["hp"]
        key = c["id"]
        out[key] = e
    json.dump(out, open(dst, "w"))
    print(f"{len(out)} cards -> {dst}")
    return out


if __name__ == "__main__":
    src = sys.argv[1] if len(sys.argv) > 1 else "cards.json"
    build(src, "hs2/standard_cards.json")
