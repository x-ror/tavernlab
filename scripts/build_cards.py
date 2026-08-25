#!/usr/bin/env python3
"""
Build ONE merged Hearthstone card array from two sources, in a single pass.

PRIMARY  -- Blizzard's card-library web API (hearthstone.blizzard.com/.../api/cards)
            It lists the cards that actually exist in the live game (~6.4k
            collectible), with art, flavor, rune cost, sideboard rules and the
            numeric ids. This is the spine of the output: one record per API
            card, and nothing that the API does not serve makes it into the
            final array.

FALLBACK -- Blizzard's official CardDefs.xml (via the `hearthstone` pip package)
            A local snapshot of every card ever defined (35k+, tokens and
            enchantments included). It is only used to FILL IN what the API
            leaves out: the string card id ("EX1_298"), the full mechanics /
            referencedMechanics tag sets, hero power, spell damage, overload,
            and any field the API returned empty or with an unresolvable id.

Join key: API `id` == CardDefs `dbfId`.

Because CardDefs is a local snapshot it goes stale: cards from the newest set
exist in the API with no CardDefs entry at all. Those are kept (they are real
cards) -- they simply carry `sources: ["blizzard"]` and no CardDefs extras.

The numeric-id -> name maps (cardTypeId, cardSetId, classId, rarityId,
minionTypeId, spellSchoolId, keywordId) are derived empirically from the join
itself, then used to turn the API's ids into readable names -- no OAuth
metadata endpoint needed. Dump them with --mappings-out.

Setup:  pip install hearthstone hearthstone-data requests

Usage:
    python scripts/build_cards.py                          # -> cards_merged.json
    python scripts/build_cards.py --sets CORE EXPERT1
    python scripts/build_cards.py --offline                # reuse the cached API dump
    python scripts/build_cards.py --include-carddefs-only  # + tokens/enchantments
    python scripts/build_cards.py --wrap --mappings-out ids.json
"""
import argparse
import json
import os
import sys
import time
from collections import Counter, defaultdict
from datetime import date

from hearthstone.cardxml import load
from hearthstone.enums import CardSet, GameTag

API = "https://hearthstone.blizzard.com/en-us/api/cards"
HEADERS = {
    # The web API expects a browser-like client.
    "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                  "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126 Safari/537.36",
    "Accept": "application/json",
}

# GameTag names treated as "mechanics" (mirrors HearthstoneJSON's extraction).
MECHANICS_TAGS = [
    "ADAPT", "ADJACENT_BUFF", "AI_MUST_PLAY", "APPEAR_FUNCTIONALLY_DEAD",
    "AURA", "AUTOATTACK", "AVENGE", "BATTLECRY", "CANT_ATTACK",
    "CANT_BE_SILENCED", "CANT_BE_TARGETED_BY_HERO_POWERS",
    "CANT_BE_TARGETED_BY_SPELLS", "CHARGE", "CHOOSE_ONE", "COLOSSAL",
    "COMBO", "CORRUPT", "COUNTER", "DEATHRATTLE", "DEATH_KNIGHT",
    "DISCOVER", "DIVINE_SHIELD", "DREDGE", "ECHO", "ELUSIVE", "ENRAGED",
    "EVIL_GLOW", "EXCAVATE", "FINALE", "FORGE", "FORGETFUL", "FREEZE",
    "GEARS", "HEROPOWER_DAMAGE", "IMMUNE", "INSPIRE", "JADE_GOLEM",
    "LIFESTEAL", "MANATHIRST", "MINIATURIZE", "MODULAR", "MORPH",
    "OUTCAST", "OVERHEAL", "OVERKILL", "OVERLOAD", "POISONOUS", "QUEST",
    "QUICKDRAW", "REBORN", "RECEIVES_DOUBLE_SPELLDAMAGE_BONUS", "RITUAL",
    "RUSH", "SECRET", "SIDEQUEST", "SILENCE", "SPARE_PART", "SPELLBURST",
    "SPELLPOWER", "START_OF_GAME", "STEALTH", "SUMMONED",
    "TAG_ONE_TURN_EFFECT", "TAUNT", "TITAN", "TOPDECK", "TRADEABLE",
    "TRIGGER_VISUAL", "TWINSPELL", "UNTOUCHABLE", "VENOMOUS", "WINDFURY",
    "ImmuneToSpellpower", "InvisibleDeathrattle",
]

# Resolve names -> GameTag enum members (skip names missing in this lib version)
TAGS = {}
for _name in MECHANICS_TAGS:
    try:
        TAGS[_name] = GameTag[_name]
    except KeyError:
        pass


def is_empty(v):
    """Treat None and empty string/list/dict as 'the source had nothing here'."""
    return v is None or (isinstance(v, (str, list, dict, tuple)) and len(v) == 0)


# --------------------------------------------------------- primary source ---

def fetch_all_cards(locale="en_US", page_size=450, delay=0.4):
    """Paginate through the whole card library."""
    import requests  # imported lazily so --offline works without the dep

    cards, page = [], 1
    while True:
        r = requests.get(API, headers=HEADERS, params={
            "class": "all", "pageSize": page_size, "page": page, "locale": locale,
        }, timeout=30)
        r.raise_for_status()
        data = r.json()
        batch = data.get("cards", [])
        cards.extend(batch)
        page_count = data.get("pageCount") or 1
        print(f"  page {page}/{page_count}: +{len(batch)} (total {len(cards)})")
        if page >= page_count or not batch:
            return cards
        page += 1
        time.sleep(delay)  # be polite


def get_api_cards(locale, cache_path, refresh, offline):
    """Fetch the library, reusing `cache_path` unless --refresh is given."""
    if cache_path and os.path.exists(cache_path) and not refresh:
        with open(cache_path, encoding="utf-8") as f:
            blob = json.load(f)
        print(f"Using cached Blizzard library: {cache_path} "
              f"({len(blob['cards'])} cards, fetched {blob.get('fetched')})")
        return blob["cards"]
    if offline:
        sys.exit(f"--offline was given but no cache at {cache_path!r}. "
                 f"Run once without --offline to populate it.")

    print("Fetching Blizzard card library...")
    cards = fetch_all_cards(locale=locale)
    print(f"Fetched {len(cards)} cards from Blizzard API")
    if cache_path:
        with open(cache_path, "w", encoding="utf-8") as f:
            json.dump({"fetched": date.today().isoformat(), "locale": locale,
                       "cards": cards}, f, ensure_ascii=False)
        print(f"Cached to {cache_path}")
    return cards


# -------------------------------------------------------- fallback source ---

def extract_mechanics(tag_dict):
    """Return sorted mechanic names present (truthy) in a card's tag dict."""
    return sorted(name for name, tag in TAGS.items() if tag_dict.get(tag))


def keyword_candidates(c):
    """Every truthy GameTag a card carries OR references, by name.

    The candidate pool for pairing up keyword ids, and deliberately wider than
    the curated MECHANICS list on both axes:

    * newer keywords (FRENZY, HONORABLE_KILL, INFUSE, the class tourists) have
      no entry in MECHANICS_TAGS and would otherwise stay numeric forever;
    * the API also tags a card with a keyword it merely mentions ("give a minion
      <b>Rush</b>"), so without referenced_tags the true partner only reaches
      ~0.5 precision and gets rejected.

    Bulk tags like CARD_SET ride along on every card and are filtered out by the
    association scoring instead.
    """
    names = {tag.name for tag, value in c.tags.items()
             if value and getattr(tag, "name", None)}
    names.update(tag.name for tag, value in c.referenced_tags.items()
                 if value and getattr(tag, "name", None))
    return list(names)


def nbsp(text):
    """CardDefs writes a non-breaking space as '_'. Every occurrence in the
    corpus sits between words ("at_least 2", "spell_this turn"), so the
    substitution is unambiguous -- and without it the text renders with
    underscores in the UI. The replacement is U+00A0, which is what the game
    (and HearthstoneJSON before it) puts there, so the text downstream stays
    byte-identical to today's corpus."""
    return text.replace("_", " ") if text else text


def enum_name(member):
    """CardDefs uses INVALID where a field is simply absent."""
    name = getattr(member, "name", None)
    return None if name in (None, "INVALID") else name


def carddefs_fields(c):
    """One CardDefs card -> flat dict of candidate fields (donor half)."""
    d = {
        "id": c.card_id,                           # string id the game logs use
        "dbfId": c.dbf_id,
        "name": c.name,
        "type": enum_name(c.type),
        "set": enum_name(c.card_set),
        "cardClass": enum_name(c.card_class),
        "classes": [cl.name for cl in c.classes],
        "rarity": enum_name(c.rarity),
        "cost": c.cost,
        "collectible": bool(c.collectible),
        "text": nbsp(c.description),
        "mechanics": extract_mechanics(c.tags),                       # on the card
        "referencedMechanics": extract_mechanics(c.referenced_tags),  # granted/referenced
    }

    # MINION TYPE (race). Not minions-only: hero forms like Lord Jaraxxus carry
    # one too, and dropping it there loses a tribe the engine checks for.
    races = [r.name for r in c.races] if c.races else []
    if not races and c.race and c.race.name != "INVALID":
        races = [c.race.name]
    if races:
        d["minionType"] = races

    t = c.type.name
    if t == "MINION":
        d["attack"] = c.atk
        d["health"] = c.health
    elif t in ("WEAPON", "LOCATION"):
        d["attack"] = c.atk
        # Both kinds have moved their durability into the HEALTH tag: every
        # weapon in the current data reports durability=0, health=N.
        d["durability"] = c.durability or c.health
    elif t in ("SPELL", "HERO_POWER"):
        ss = c.spell_school                        # SPELL SCHOOL
        d["spellSchool"] = ss.name if ss and ss.name != "NONE" else None
    elif t == "HERO":
        d["health"] = c.health
        d["armor"] = c.armor

    # Extras the web API does not expose at all.
    if c.hero_power:
        d["heroPowerId"] = c.hero_power
    if c.spell_damage:
        d["spellDamage"] = c.spell_damage
    if c.overload:
        d["overload"] = c.overload
    if getattr(c, "targeting_arrow_text", None):
        d["targetingArrowText"] = c.targeting_arrow_text
    if c.elite:
        d["elite"] = True
    if getattr(c, "flavortext", None):
        d["flavorText"] = nbsp(c.flavortext)
    if getattr(c, "artist", None):
        d["artist"] = c.artist
    return d


def load_carddefs(locale):
    db, _ = load(locale=locale)
    return {c.dbf_id: c for c in db.values()}


# ------------------------------------------------------------- id mapping ---

def majority_map(pairs):
    """{numeric_id: most common name seen with it} from (id, name) pairs.

    For the single-valued fields (type, set, class, rarity, ...) one card is one
    unambiguous vote, so a plain majority is enough.
    """
    votes = defaultdict(Counter)
    for k, v in pairs:
        if k is not None and v:
            votes[k][v] += 1
    return {k: c.most_common(1)[0][0] for k, c in sorted(votes.items())}


def associate_map(observations, min_precision=0.6, min_recall=0.3):
    """{numeric_id: name} for LIST-valued fields, where one card carries several
    ids and several names and the pairing is unknown.

    `observations` is a list of (ids, names) per card. A card with 3 keyword ids
    and 4 mechanics gives no direct pairing, but across thousands of cards the
    true partner of id k is the mechanic that shows up in (nearly) every card
    carrying k and hardly ever without it. Background tags like TRIGGER_VISUAL
    ride along on many cards, so they score high on precision but near-zero on
    recall and get rejected -- which a plain majority vote cannot do.
    """
    id_count, name_count = Counter(), Counter()
    both = defaultdict(Counter)
    for ids, names in observations:
        for i in set(ids):
            id_count[i] += 1
            for n in set(names):
                both[i][n] += 1
        for n in set(names):
            name_count[n] += 1

    out = {}
    for i, counts in sorted(both.items()):
        best, best_score = None, 0.0
        for n, c in counts.items():
            precision = c / id_count[i]          # how often k implies m
            recall = c / name_count[n]           # how often m implies k
            score = precision * recall
            if precision >= min_precision and recall >= min_recall and score > best_score:
                best, best_score = n, score
        if best:
            out[i] = best
    return out


def derive_mappings(api_cards, by_dbf):
    """Learn numeric-id -> name maps by joining the two sources.

    Single-valued fields are settled by majority vote over unambiguous cards:
    single-class cards teach classId, single-race minions teach minionTypeId.
    keywordId is list-valued on both sides, so it goes through associate_map
    instead. The multi-value id lists (multiClassIds, multiTypeIds) are then
    resolved with the very same maps, since they share the id space.
    """
    type_p, set_p, class_p, rarity_p, race_p, school_p = [], [], [], [], [], []
    kw_obs, dual_race = [], []

    for api in api_cards:
        donor = by_dbf.get(api["id"])
        if donor is None:
            continue
        d = carddefs_fields(donor)
        type_p.append((api.get("cardTypeId"), d["type"]))
        set_p.append((api.get("cardSetId"), d["set"]))
        rarity_p.append((api.get("rarityId"), d["rarity"]))
        school_p.append((api.get("spellSchoolId"), d.get("spellSchool")))
        if len(d["classes"]) == 1:
            class_p.append((api.get("classId"), d["cardClass"]))
        races = d.get("minionType") or []
        if len(races) == 1:
            race_p.append((api.get("minionTypeId"), races[0]))
        elif len(races) == 2 and len(api.get("multiTypeIds") or []) == 1:
            dual_race.append((api, races))
        kws = api.get("keywordIds") or []
        if kws:
            kw_obs.append((kws, keyword_candidates(donor)))

    maps = {
        "note": "Derived empirically by joining Blizzard API ids with CardDefs enum names",
        "cardTypeId": majority_map(type_p),
        "cardSetId": majority_map(set_p),
        "classId": majority_map(class_p),
        "rarityId": majority_map(rarity_p),
        "minionTypeId": majority_map(race_p),
        "spellSchoolId": majority_map(school_p),
        "keywordId": associate_map(kw_obs),
    }

    # Second pass: dual-type minions teach the ids the single-type pass missed.
    # The primary minionTypeId is already known, so the other race must be the
    # one behind multiTypeIds[0].
    extra = []
    for api, races in dual_race:
        primary = maps["minionTypeId"].get(api.get("minionTypeId"))
        rest = [r for r in races if r != primary]
        if primary and len(rest) == 1:
            extra.append((api["multiTypeIds"][0], rest[0]))
    if extra:
        for k, v in majority_map(extra).items():
            maps["minionTypeId"].setdefault(k, v)
    return maps


# ------------------------------------------------------------------ merge ---

def build_record(api, maps, keep_ids):
    """Blizzard API card -> flat record. This is the spine of every entry."""
    type_name = maps["cardTypeId"].get(api.get("cardTypeId"))
    class_map, race_map = maps["classId"], maps["minionTypeId"]

    classes = [class_map[i] for i in ([api.get("classId")] + list(api.get("multiClassIds") or []))
               if i in class_map]
    d = {
        "id": None,                                # CardDefs fills the string id
        "dbfId": api.get("id"),
        "name": api.get("name"),
        "slug": api.get("slug"),
        "type": type_name,
        "set": maps["cardSetId"].get(api.get("cardSetId")),
        "cardClass": class_map.get(api.get("classId")),
        "classes": list(dict.fromkeys(classes)),
        "rarity": maps["rarityId"].get(api.get("rarityId")),
        "cost": api.get("manaCost"),
        "collectible": bool(api.get("collectible")),
        "text": api.get("text"),
        "mechanics": [],                           # CardDefs fills these
        "referencedMechanics": [],
    }

    if type_name == "MINION":
        d["attack"] = api.get("attack")
        d["health"] = api.get("health")
        races = [race_map[i] for i in ([api.get("minionTypeId")] + list(api.get("multiTypeIds") or []))
                 if i in race_map]
        d["minionType"] = list(dict.fromkeys(races))
    elif type_name == "WEAPON":
        d["attack"] = api.get("attack")
        d["durability"] = api.get("health")        # the API reports it as health
    elif type_name == "LOCATION":
        d["durability"] = api.get("health")
    elif type_name == "HERO":
        d["health"] = api.get("health")
        if api.get("armor") is not None:
            d["armor"] = api["armor"]
    if api.get("spellSchoolId") is not None:
        d["spellSchool"] = maps["spellSchoolId"].get(api["spellSchoolId"])

    # Art and text flavour -- the API is the only source of the live art URLs.
    for key in ("image", "imageGold", "cropImage", "flavorText"):
        if api.get(key):
            d[key] = api[key]
    if api.get("artistName"):
        d["artist"] = api["artistName"]

    # Gameplay data only the live API knows about.
    kws = api.get("keywordIds") or []
    if kws:
        d["keywords"] = [maps["keywordId"].get(k, k) for k in kws]
    for src, dst in (("childIds", "childIds"),            # tokens this card creates
                     ("copyOfCardId", "copyOfCardId"),
                     ("parentId", "parentId"),
                     ("bundledCardIds", "bundledCardIds"),
                     ("runeCost", "runeCost"),            # Death Knight runes
                     ("sideboard", "sideboard"),
                     ("bannedFromSideboard", "bannedFromSideboard"),
                     ("deckSizeMod", "deckSizeMod"),
                     ("factionId", "factionIds"),
                     ("isZilliaxFunctionalModule", "zilliaxFunctionalModule"),
                     ("isZilliaxCosmeticModule", "zilliaxCosmeticModule")):
        if api.get(src) not in (None, [], {}, False):
            d[dst] = api[src]
    if api.get("touristClassId") is not None:
        d["touristClass"] = class_map.get(api["touristClassId"], api["touristClassId"])

    if keep_ids:
        d["blizzardIds"] = {
            "cardTypeId": api.get("cardTypeId"),
            "cardSetId": api.get("cardSetId"),
            "classId": api.get("classId"),
            "multiClassIds": api.get("multiClassIds", []),
            "minionTypeId": api.get("minionTypeId"),
            "multiTypeIds": api.get("multiTypeIds", []),
            "spellSchoolId": api.get("spellSchoolId"),
            "rarityId": api.get("rarityId"),
            "keywordIds": kws,
        }

    d["sources"] = ["blizzard"]
    return d


def fill_from_carddefs(record, donor_card):
    """Fill only what the API left out. The API's own values always win."""
    donor = carddefs_fields(donor_card)
    filled = []
    for key, value in donor.items():
        if key in ("dbfId", "collectible") or is_empty(value):
            continue
        if is_empty(record.get(key)):
            record[key] = value
            filled.append(key)
    record["sources"].append("carddefs")
    return filled


# ------------------------------------------------------------------- main ---

def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sets", nargs="*", default=None,
                    help="CardSet names to keep (e.g. VANILLA CORE EXPERT1). Default: all.")
    ap.add_argument("--locale", default="enUS", help="CardDefs locale, e.g. enUS, ruRU.")
    ap.add_argument("--out", default="cards_merged.json")
    ap.add_argument("--api-cache", default="blizzard_api_cache.json",
                    help="Reuse/store the raw API response here (pass '' to disable).")
    ap.add_argument("--refresh", action="store_true", help="Ignore the API cache and refetch.")
    ap.add_argument("--offline", action="store_true",
                    help="Never hit the network; fail if the cache is missing.")
    ap.add_argument("--include-carddefs-only", action="store_true",
                    help="Also append cards only CardDefs has (tokens, enchantments, hero "
                         "powers). Off by default: the API defines what is in the game.")
    ap.add_argument("--keep-ids", action="store_true",
                    help="Keep Blizzard's raw numeric ids on each card under 'blizzardIds'.")
    ap.add_argument("--wrap", action="store_true",
                    help="Emit {meta, enums, blizzardIdMappings, cards} instead of a bare array.")
    ap.add_argument("--mappings-out", default=None,
                    help="Also write the derived numeric-id -> name maps here.")
    args = ap.parse_args()

    wanted_sets = {CardSet[s.upper()].name for s in args.sets} if args.sets else None

    # --- primary source -------------------------------------------------
    api_locale = args.locale if "_" in args.locale else args.locale[:2] + "_" + args.locale[2:]
    api_cards = get_api_cards(api_locale, args.api_cache or None, args.refresh, args.offline)

    # --- fallback source ------------------------------------------------
    print("Loading CardDefs.xml...")
    by_dbf = load_carddefs(args.locale)
    print(f"Loaded {len(by_dbf)} cards from CardDefs")

    # --- learn the id vocabulary from the overlap -----------------------
    maps = derive_mappings(api_cards, by_dbf)

    # --- build one record per API card ----------------------------------
    cards, enriched, api_only, used = [], 0, 0, set()
    fill_stats = Counter()
    for api in api_cards:
        record = build_record(api, maps, args.keep_ids)
        donor = by_dbf.get(api["id"])
        if donor is not None:
            fill_stats.update(fill_from_carddefs(record, donor))
            enriched += 1
            used.add(api["id"])
        else:
            api_only += 1        # newer than the local CardDefs snapshot
            # No CardDefs entry means no string card id. Fall back to the slug
            # so consumers that key by `id` get a unique key instead of null --
            # the game's logs will never emit it, which is the honest outcome
            # for a card this snapshot cannot describe.
            record["id"] = record["slug"]
        cards.append(record)

    # Optional: cards that exist only in CardDefs (tokens, enchantments, ...).
    carddefs_only = 0
    if args.include_carddefs_only:
        for dbf, c in by_dbf.items():
            if dbf in used:
                continue
            record = carddefs_fields(c)
            record["sources"] = ["carddefs"]
            cards.append(record)
            carddefs_only += 1

    if wanted_sets:
        cards = [c for c in cards if c.get("set") in wanted_sets]

    cards.sort(key=lambda d: (d.get("set") or "", d.get("type") or "",
                              d.get("cardClass") or "", d.get("cost") or 0,
                              d.get("name") or ""))

    # --- report / write --------------------------------------------------
    counts_by_type, counts_by_set = Counter(), Counter()
    unresolved = Counter()
    for c in cards:
        counts_by_type[c.get("type")] += 1
        counts_by_set[c.get("set")] += 1
        for field in ("type", "set", "cardClass", "rarity"):
            if c.get(field) is None:
                unresolved[field] += 1
    # keyword ids with no CardDefs tag to pair with stay numeric on the card
    unresolved_kw = sorted({k for c in cards for k in (c.get("keywords") or [])
                            if not isinstance(k, str)})

    if args.wrap:
        enums = {
            "types": sorted({c["type"] for c in cards if c.get("type")}),
            "sets": sorted({c["set"] for c in cards if c.get("set")}),
            "spellSchools": sorted({c["spellSchool"] for c in cards if c.get("spellSchool")}),
            "minionTypes": sorted({r for c in cards for r in (c.get("minionType") or [])}),
            "keywords": sorted({str(k) for c in cards for k in (c.get("keywords") or [])}),
            "mechanics": sorted({m for c in cards for m in (c.get("mechanics") or [])}),
            "referencedMechanics": sorted({m for c in cards
                                           for m in (c.get("referencedMechanics") or [])}),
            "rarities": sorted({c["rarity"] for c in cards if c.get("rarity")}),
            "classes": sorted({c["cardClass"] for c in cards if c.get("cardClass")}),
        }
        out = {
            "meta": {
                "primarySource": "hearthstone.blizzard.com card-library API",
                "fallbackSource": "hearthstone-data (official CardDefs.xml)",
                "locale": args.locale,
                "generated": date.today().isoformat(),
                "filterSets": args.sets or "ALL",
                "totalCards": len(cards),
                "apiCards": len(api_cards),
                "enrichedByCardDefs": enriched,
                "apiOnly": api_only,
                "cardDefsOnly": carddefs_only,
                "unresolvedFields": dict(unresolved),
                "unresolvedKeywordIds": unresolved_kw,
                "countsByType": dict(sorted(counts_by_type.items(), key=lambda kv: str(kv[0]))),
                "countsBySet": dict(sorted(counts_by_set.items(), key=lambda kv: str(kv[0]))),
            },
            "enums": enums,
            "blizzardIdMappings": maps,
            "cards": cards,
        }
    else:
        out = cards  # ONE flat array

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, indent=2, ensure_ascii=False)

    if args.mappings_out:
        with open(args.mappings_out, "w", encoding="utf-8") as f:
            json.dump(maps, f, indent=2, ensure_ascii=False)
        print(f"Wrote id mappings -> {args.mappings_out}")

    print(f"\n{len(api_cards)} live cards from the API; {enriched} enriched with CardDefs, "
          f"{api_only} newer than the local CardDefs snapshot.")
    if carddefs_only:
        print(f"Appended {carddefs_only} CardDefs-only cards (tokens/enchantments/hero powers).")
    if unresolved:
        print(f"Fields left unresolved: {dict(unresolved)}")
    if unresolved_kw:
        print(f"{len(unresolved_kw)} keyword ids had no CardDefs tag to pair with "
              f"and stay numeric: {unresolved_kw}")
    top = ", ".join(f"{k}={v}" for k, v in fill_stats.most_common(6))
    print(f"Most-filled CardDefs fields: {top}")
    print(f"Wrote {len(cards)} cards -> {args.out}")


if __name__ == "__main__":
    main()
