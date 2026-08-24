"""A copied gauntlet deck has to import as that deck.

The trap this pins: `decks.load_meta()` folds a sideboard into the main
list — ten copies of Commander Beatrix's minion — which is right for the
simulator and would be an **illegal 30-card list** in the game. Encoding
a deckstring from that flattened `Deck` would hand the player a code
that imports something they never saw.

`decks.export_gauntlet()` therefore encodes from the raw gauntlet file,
and marks any deck whose sideboard it cannot represent.
"""
import pytest

from hs2 import carddata, decks, deckstring, formats

FORMATS = [formats.STANDARD, formats.WILD]


@pytest.fixture(scope="module", params=FORMATS)
def exported(request):
    fmt = request.param
    out = decks.export_gauntlet(fmt)
    if not out:
        pytest.skip(f"no gauntlet built for {fmt}")
    return fmt, out


def test_every_gauntlet_deck_exports(exported):
    fmt, out = exported
    assert len(out) == len(decks.load_meta(fmt)), \
        "a deck in the gauntlet produced no deckstring"


def test_the_code_carries_a_hero_and_the_stated_count(exported):
    """Deck size is whatever the gauntlet says. Thief Priest is a
    20-card build; asserting 30 here would encode a guess about the
    game's rules into the test suite."""
    import json
    fmt, out = exported
    raw = json.load(open(decks.gauntlet_path(fmt), encoding="utf-8"))
    for name, entry in out.items():
        info = deckstring.decode(entry["code"])
        total = sum(n for _dbf, n in info["cards"])
        assert len(info["heroes"]) == 1, name
        assert total == entry["cards"], name
        assert total == sum(n for _cn, n in raw[name]["cards"]), name


def test_the_code_carries_the_right_format(exported):
    fmt, out = exported
    want = 1 if fmt == formats.WILD else 2
    for name, entry in out.items():
        assert deckstring.decode(entry["code"])["format"] == want, name


def test_the_code_resolves_to_the_same_class(exported):
    from evaluate import try_resolve
    fmt, out = exported
    by_name = {d.name: d for d in decks.load_meta(fmt)}
    for name, entry in out.items():
        _deck, info = try_resolve(entry["code"])
        assert info.get("error") is None, f"{name}: {info.get('error')}"
        assert info["cls"] == by_name[name].cls, name


def test_the_cards_survive_the_round_trip(exported):
    """Decode the code back and compare it to the gauntlet file itself,
    not to `load_meta` — that is the whole point of the exercise."""
    import json
    import os
    fmt, out = exported
    carddata.ensure_defs(fmt)
    by_dbf = {d.dbf: d.name for d in carddata.DEFS.values()}
    raw = json.load(open(decks.gauntlet_path(fmt), encoding="utf-8"))

    for name, entry in out.items():
        want = {cn: n for cn, n in raw[name]["cards"]}
        got = {}
        for dbf, n in deckstring.decode(entry["code"])["cards"]:
            got[by_dbf[dbf]] = got.get(by_dbf[dbf], 0) + n
        assert got == want, name


def test_a_sideboard_deck_is_flagged_incomplete(exported):
    """Not silently shipped as if it were the whole deck."""
    import json
    fmt, out = exported
    raw = json.load(open(decks.gauntlet_path(fmt), encoding="utf-8"))
    for name, entry in out.items():
        if raw[name].get("sideboard"):
            assert not entry["complete"], name


def test_the_flattened_deck_would_have_been_wrong(exported):
    """Why this module exists: `load_meta` is not a legal decklist.

    If this ever starts passing for every deck, the sideboard folding in
    `load_meta` is gone and `export_gauntlet` could be simplified.
    """
    fmt, _out = exported
    sizes = {d.name: len(d.card_ids) for d in decks.load_meta(fmt)}
    if all(n == 30 for n in sizes.values()):
        pytest.skip("no deck is flattened in this gauntlet")
    assert any(n != 30 for n in sizes.values())
