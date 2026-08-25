"""Which cards are legal where, and which game modes we actually support.

Two independent axes, and conflating them is the usual mistake:

* **format** — Standard / Wild / Twist. Decides the legal card pool.
* **mode** — ranked / casual / arena / friendly. Decides how a game is
  scored. You play *ranked* **in** Standard or **in** Wild.

`store/schema.sql` already models both (`games.mode`, `games.format`);
this module is where the card-pool half is decided, so the set lists live
in exactly one place instead of being re-derived per script.

The `hearthstone` package ships `CardSet.craftable`, the closest thing to
an official "is this a real constructed set" flag, but it lags releases —
it is missing Stormwind, Nathria and Galakrond at the pinned version. So
it seeded `WILD_SETS` once and the gaps were named explicitly; the list
below is that result, written out.

Written out and not computed at import, deliberately: this module sits
under `carddata`, which every worker process and every frozen build
loads, and it has no business importing a card-database package to learn
a list of 46 strings. `tests/test_formats.py` recomputes it from
`CardSet.craftable` and fails if the two ever disagree, so the constant
cannot drift silently.
"""

# Standard rotates every year. `hearthstone` cannot know the current
# rotation, so this is ours to maintain — it is the one list that has a
# calendar attached to it.
STANDARD_SETS = frozenset({
    "CORE", "EMERALD_DREAM", "THE_LOST_CITY", "TIME_TRAVEL",
    "CATACLYSM", "ESCAPEFROM_VIOLET_HOLD", "EVENT", "PATH_OF_ARTHAS",
})

# Real constructed sets that `craftable` does not list at the pinned
# version. Verified against the corpus rather than assumed.
_CRAFTABLE_GAPS = frozenset({
    "CORE",         # the free rotating Core set
    "CORE_HIDDEN",  # Core cards currently out of rotation - still Wild
    "EXPERT1",      # the original Classic set
    "EVENT",        # event-only cards, playable in constructed
    "STORMWIND",
    "REVENDRETH",
    "YEAR_OF_THE_DRAGON",
})

# Collectible in the data, not playable in any live format.
EXCLUDED_SETS = frozenset({
    # The Classic format was retired. Every VANILLA card is a reprint of
    # an EXPERT1 card (243 of its 244 names overlap), so keeping it would
    # duplicate the whole Classic pool under a second set code.
    "VANILLA",
})


#: `{s.name for s in CardSet if s.craftable} | _CRAFTABLE_GAPS`
#: minus `EXCLUDED_SETS`, materialised. Pinned by `test_formats.py`.
WILD_SETS = frozenset({
    "ALTERAC_VALLEY", "BATTLE_OF_THE_BANDS", "BLACK_TEMPLE",
    "BOOMSDAY", "BRM", "CATACLYSM", "CORE", "CORE_HIDDEN", "DALARAN",
    "DARKMOON_FAIRE", "DEMON_HUNTER_INITIATE", "DRAGONS",
    "EMERALD_DREAM", "ESCAPEFROM_VIOLET_HOLD", "EVENT", "EXPERT1",
    "GANGS", "GILNEAS", "GVG", "ICECROWN", "ISLAND_VACATION", "KARA",
    "LEGACY", "LOE", "LOOTAPALOOZA", "NAXX", "OG", "PATH_OF_ARTHAS",
    "RETURN_OF_THE_LICH_KING", "REVENDRETH", "SCHOLOMANCE", "SPACE",
    "STORMWIND", "TGT", "THE_BARRENS", "THE_LOST_CITY",
    "THE_SUNKEN_CITY", "TIME_TRAVEL", "TITANS", "TROLL", "ULDUM",
    "UNGORO", "WHIZBANGS_WORKSHOP", "WILD_WEST", "WONDERS",
    "YEAR_OF_THE_DRAGON",
})

# Card types that can go in a deck. HERO belongs here: hero *cards*
# (Deathwing Worldbreaker, the Death Knight heroes) are played from hand
# like any other. Hero *skins* share the type and are collectible too,
# but they live in HERO_SKINS, which is legal in no format - so the set
# list excludes them and the type list does not have to.
DECK_TYPES = frozenset({"MINION", "SPELL", "WEAPON", "LOCATION", "HERO"})

STANDARD = "standard"
WILD = "wild"
FORMATS = (STANDARD, WILD)
# TODO(twist): Twist runs on a rotating, season-specific pool that is not
# derivable from set codes. It needs its own per-season card list.

RANKED = "ranked"
# TODO(modes): casual / friendly / arena. Ranked is the only mode whose
# rules we model; the others differ in scoring and, for arena, in how a
# deck comes to exist at all. `games.mode` already records them.
MODES = (RANKED,)


# Deckstrings carry the format as a byte (`hearthstone.enums.FormatType`).
DECKSTRING_FORMATS = {1: WILD, 2: STANDARD}
# 0 = unknown, 3 = the retired Classic format, 4 = Twist (see the TODO
# above). None of the three names a pool we can build.


def from_deckstring(code):
    """Format byte from a decoded deckstring -> our name, or None."""
    return DECKSTRING_FORMATS.get(code)


def sets_for(fmt):
    """The legal set codes for a format."""
    if fmt == STANDARD:
        return STANDARD_SETS
    if fmt == WILD:
        return WILD_SETS
    raise ValueError(f"unknown format: {fmt!r} (have {FORMATS})")


def is_legal(card, fmt):
    """Is this card definition legal in `fmt`?

    Takes anything with `.set` and `.type` — a `CardDef` or a raw corpus
    entry via `entry_is_legal`.
    """
    return card.type in DECK_TYPES and card.set in sets_for(fmt)


def entry_is_legal(entry, fmt):
    """`is_legal` for a raw card dict, as the corpus builder sees it."""
    return (entry.get("type") in DECK_TYPES
            and entry.get("set") in sets_for(fmt))


def formats_of(card):
    """Every format this card is legal in, most restrictive first."""
    return tuple(f for f in FORMATS if is_legal(card, f))
