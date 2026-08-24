"""Deck codes arrive wrapped in whatever the site printed around them.

Nobody copies a bare deckstring. HSReplay, the in-game export and every
tracker hand out a commented block, and pasting it whole used to fail
inside base64 with "string argument should contain only ASCII
characters" — true about the Cyrillic comment lines, useless to a
player.

`hs2.deckstring.extract` therefore has to find the code in the noise,
and — just as important — has to refuse clearly when there is no code.
"""
import pytest

from hs2 import deckstring

# A real Zee Shaman export, comments and trailing links intact.
PASTE = """### Zee Shaman
# Класс: Шаман
# Формат: Стандартный
#
# 2x (0) Ученица ведьмы
# 2x (1) Веселая спутница
# 1x (2) Страж Майев
# 1x (10) Смертокрыл Разрушитель миров
#
AAECAaoICsmeBsODB9C/B/nDB4LUB5vUB8/bB9DbB4jdB9/lBwqe1ATt5gbgnQexsAePvge1wAfJwAfJ2wfI5Qfm/QcAAA==
# Для переноса колоды в игру, скопируйте, затем нажмите "Новая колода" в Hearthstone
# Колода доступна здесь: https://hsreplay.net/decks/JKdwO6DG2QHXLuegIjbuUg/#gameType=RANKED_STANDARD
"""

BARE = ("AAECAaoICsmeBsODB9C/B/nDB4LUB5vUB8/bB9DbB4jdB9/lBwqe1ATt5gbg"
        "nQexsAePvge1wAfJwAfJ2wfI5Qfm/QcAAA==")


def test_a_bare_deckstring_still_works():
    assert deckstring.extract(BARE) == BARE


def test_the_pasted_block_yields_the_same_code():
    assert deckstring.extract(PASTE) == BARE


def test_the_block_decodes_to_the_same_deck_as_the_bare_code():
    assert deckstring.decode(PASTE) == deckstring.decode(BARE)


def test_the_decoded_deck_is_the_real_one():
    info = deckstring.decode(PASTE)
    assert info["format"] == 2, "Standard"
    assert len(info["heroes"]) == 1
    assert sum(n for _dbf, n in info["cards"]) == 30


@pytest.mark.parametrize("wrapper", [
    "%s",
    "  %s  ",
    "\n\n%s\n\n",
    "### Deck\n%s",
    "%s\n# Колода доступна здесь: https://hsreplay.net/decks/abc/",
])
def test_surrounding_whitespace_and_comments_are_ignored(wrapper):
    assert deckstring.extract(wrapper % BARE) == BARE


def test_a_code_wrapped_across_lines_is_rejoined():
    """Some exports hard-wrap the base64."""
    split = BARE[:40] + "\n" + BARE[40:]
    assert deckstring.extract(split) == BARE


def test_comment_lines_are_never_mistaken_for_the_code():
    """A `#` line can hold a base64-looking word. Candidates are checked
    by parsing them, not by how they look."""
    noisy = ("### Deck\n"
             "# AAECAQcG9wSStAK1BMDBAvUFrwQMtwSNAY4Bqwm=\n"
             "# see https://example.test/AAECAaoICsmeBsODB9C\n"
             + BARE + "\n")
    assert deckstring.extract(noisy) == BARE


def test_comments_without_a_code_say_so():
    only_comments = "### Zee Shaman\n# Класс: Шаман\n# 2x (0) Ученица\n"
    with pytest.raises(ValueError) as exc:
        deckstring.extract(only_comments)
    assert "деккод" in str(exc.value)


@pytest.mark.parametrize("junk", ["", "   ", "\n", "не деккод", "AAEC"])
def test_junk_is_refused_with_a_readable_message(junk):
    with pytest.raises(ValueError) as exc:
        deckstring.extract(junk)
    msg = str(exc.value)
    assert "деккод" in msg
    # Never leak base64's own complaint to the player.
    assert "ASCII" not in msg and "base64" not in msg.lower()


def test_the_deck_name_is_recovered_from_the_paste():
    assert deckstring.deck_name(PASTE) == "Zee Shaman"
    assert deckstring.deck_name(BARE) is None


def test_try_resolve_accepts_the_paste():
    """The whole point: what the player copied resolves to a deck."""
    from evaluate import try_resolve
    _deck, pasted = try_resolve(PASTE)
    _deck, bare = try_resolve(BARE)
    assert pasted.get("error") is None, pasted.get("error")
    for key in ("cls", "total", "format"):
        assert pasted[key] == bare[key]
