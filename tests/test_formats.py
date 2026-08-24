"""Format legality is a data-driven guess until something pins it.

`hearthstone.CardSet.craftable` seeds the Wild list but lags releases, and
the Standard list rotates yearly by hand. Both are exactly the kind of
constant that goes stale silently, so these tests name real cards and
assert where they land.
"""
import json
import os

import pytest

from hs2 import formats as F

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WILD_CORPUS = os.path.join(ROOT, "hs2", "wild_cards.json")


def test_standard_is_a_subset_of_wild():
    """Anything legal in Standard is legal in Wild. Never the reverse."""
    assert F.STANDARD_SETS <= F.WILD_SETS


def test_the_retired_classic_set_is_excluded():
    """VANILLA is a full reprint of EXPERT1 (243 of its 244 names). Keeping
    it would duplicate the entire Classic pool under a second set code."""
    assert "VANILLA" in F.EXCLUDED_SETS
    assert "VANILLA" not in F.WILD_SETS
    assert "EXPERT1" in F.WILD_SETS


def test_out_of_rotation_core_is_wild_not_standard():
    """CORE_HIDDEN holds real cards (Chillwind Yeti, Sprint) that left the
    Core rotation. They are playable in Wild and must not be dropped."""
    assert "CORE_HIDDEN" in F.WILD_SETS
    assert "CORE_HIDDEN" not in F.STANDARD_SETS


@pytest.mark.parametrize("missed", ["STORMWIND", "REVENDRETH",
                                    "YEAR_OF_THE_DRAGON"])
def test_sets_craftable_forgets_are_still_wild(missed):
    """The pinned `hearthstone` release omits these from `craftable`; they
    are real constructed sets and a Wild deck may use them."""
    assert missed in F.WILD_SETS


def test_hero_skins_are_excluded_but_hero_cards_are_not():
    """Both are type HERO and both are `collectible`. The set is what
    separates a portrait from Deathwing, Worldbreaker — which is a card
    you put in a deck, and was briefly legal in no format at all."""
    assert "HERO" in F.DECK_TYPES
    assert "HERO_SKINS" not in F.WILD_SETS
    assert "HERO_SKINS" not in F.STANDARD_SETS
    assert not F.is_legal(_Card("HERO_SKINS", "HERO"), F.WILD)
    assert F.is_legal(_Card("CATACLYSM", "HERO"), F.STANDARD)


def test_unknown_format_is_an_error_not_an_empty_pool():
    with pytest.raises(ValueError):
        F.sets_for("twist")


def test_deckstring_format_byte_maps_to_a_pool():
    assert F.from_deckstring(1) == F.WILD
    assert F.from_deckstring(2) == F.STANDARD
    # unknown / Classic / Twist name no pool we can build
    for code in (0, 3, 4, 99):
        assert F.from_deckstring(code) is None


class _Card:
    def __init__(self, set_, type_="MINION"):
        self.set, self.type = set_, type_


def test_is_legal_checks_both_set_and_type():
    core = _Card("CORE")
    assert F.is_legal(core, F.STANDARD) and F.is_legal(core, F.WILD)
    old = _Card("GVG")
    assert not F.is_legal(old, F.STANDARD) and F.is_legal(old, F.WILD)
    # a portrait: right type, wrong set
    assert not F.is_legal(_Card("HERO_SKINS", "HERO"), F.STANDARD)
    # a hero power: right set, wrong type — it is never in a deck
    assert not F.is_legal(_Card("CORE", "HERO_POWER"), F.STANDARD)


def test_formats_of_orders_standard_first():
    assert F.formats_of(_Card("CORE")) == (F.STANDARD, F.WILD)
    assert F.formats_of(_Card("GVG")) == (F.WILD,)
    assert F.formats_of(_Card("VANILLA")) == ()


def test_only_ranked_is_modelled():
    """Format and mode are different axes; ranked is the only mode whose
    rules we claim to model. The rest are recorded, not simulated."""
    assert F.MODES == (F.RANKED,)


@pytest.mark.skipif(not os.path.exists(WILD_CORPUS),
                    reason="Wild delta not built (build_data.py --format wild)")
class TestAgainstTheBuiltCorpus:
    """Only runs where someone actually built the Wild delta."""

    @staticmethod
    def _corpus():
        with open(WILD_CORPUS, encoding="utf-8") as fh:
            return json.load(fh)

    def test_the_delta_holds_no_standard_cards(self):
        """It is a delta. A Standard-legal entry in here would be loaded
        twice and shadow the shipped one."""
        from hs2 import carddata
        carddata.build_defs(F.STANDARD)
        std_ids = set(carddata.DEFS)
        overlap = std_ids & set(self._corpus())
        assert not overlap, sorted(overlap)[:10]
        carddata.build_defs(F.STANDARD)   # leave the session as we found it

    def test_the_delta_carries_no_retired_sets(self):
        bad = {c["set"] for c in self._corpus().values()} & F.EXCLUDED_SETS
        assert not bad, bad


def test_a_name_resolves_to_the_card_not_the_portrait():
    """Ten collectible cards share a name with a hero portrait.

    The portrait has no cost and no text, and because behaviours are also
    matched by name it inherits the real card's code while keeping the
    blank stats. Three gauntlet decks were built out of such blanks, and
    `Deck.from_names` — which every gauntlet JSON goes through — is what
    hit it.
    """
    from hs2 import carddata
    carddata.ensure_defs(F.STANDARD)
    for name in ("Irida Sinseeker", "Broxigar", "Garona Halforcen",
                 "Husk, Eternal Reaper"):
        d = carddata.get_def(name)
        assert d.set != "HERO_SKINS", f"{name} resolved to a portrait"
        assert d.cost > 0, f"{name} resolved to a 0-cost blank"


def test_hero_powers_still_resolve_to_their_own_printing():
    """HERO_SKINS also catalogues the basic hero powers, so the portrait
    rule is scoped to type HERO. Demoting the whole set moved nine of
    these and changed how games play."""
    from hs2 import carddata
    carddata.ensure_defs(F.STANDARD)
    for name in ("Lesser Heal", "Life Tap", "Armor Up!", "Steady Shot"):
        d = carddata.get_def(name)
        assert d.type == "HERO_POWER", f"{name} -> {d.type}"


def test_the_wild_set_list_still_matches_its_source():
    """`hs2/formats.py` writes WILD_SETS out instead of computing it, so
    that `carddata` — and therefore every pool worker and every frozen
    build — does not import a card-database package to learn 46 strings.

    Written-out constants rot. This recomputes it and fails on drift, so
    a `hearthstone` upgrade that adds a set is a red test, not a silent
    hole in the Wild pool.
    """
    from hearthstone.enums import CardSet
    seed = {s.name for s in CardSet if s.craftable}
    expected = (seed | F._CRAFTABLE_GAPS) - F.EXCLUDED_SETS
    missing, extra = expected - F.WILD_SETS, F.WILD_SETS - expected
    assert not missing and not extra, (
        f"WILD_SETS drifted — missing {sorted(missing)}, "
        f"extra {sorted(extra)}. Update the literal in hs2/formats.py.")
