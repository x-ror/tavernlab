"""Minimal Hearthstone deckstring decoder (no external dependencies).

Format: base64 of [0x00, version, format, heroes, 1x-cards, 2x-cards,
nx-cards, (optional sideboard section)]. All numbers are varints.

`decode()` accepts what the user actually has on the clipboard, not just
the bare code. Every deck site (HSReplay, out-of-game collection export)
hands out a commented block:

    ### Zee Shaman
    # Класс: Шаман
    # 2x (0) Ученица ведьмы
    AAECAaoICsmeBsODB9C/B...
    # Колода доступна здесь: https://hsreplay.net/decks/...

Pasted whole, that used to die inside base64 with "string argument
should contain only ASCII characters" — a true statement about the
Cyrillic comments and a useless one to a player.
"""
import base64
import re


class _Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def varint(self):
        shift = 0
        result = 0
        while True:
            if self.pos >= len(self.data):
                raise ValueError("truncated deckstring")
            b = self.data[self.pos]
            self.pos += 1
            result |= (b & 0x7F) << shift
            if not (b & 0x80):
                return result
            shift += 7

    @property
    def eof(self):
        return self.pos >= len(self.data)


def _plausible(code):
    """Parse `code` if it really is a deckstring, else None.

    Validating by *parsing* rather than by shape is the point: a deck
    site's block also contains base64-looking words, and a length check
    would happily pick one of them.
    """
    try:
        info = _parse(base64.b64decode(code, validate=True))
    except Exception:
        return None
    # A deck has a hero and cards. Short random base64 can survive the
    # varint reader by accident; it cannot survive this.
    if info["version"] != 1 or not info["heroes"] or not info["cards"]:
        return None
    return info


# `###` titles the deck, `#` comments a card line. Blizzard's own export
# and every site that copies it use both.
_COMMENT = re.compile(r"^\s*#")


def extract(text):
    """The deckstring inside whatever the user pasted.

    Raises ValueError naming the real problem, so the UI can repeat it to
    the player instead of relaying a base64 complaint.
    """
    text = (text or "").strip()
    if not text:
        raise ValueError("порожній деккод")
    if _plausible(text) is not None:          # already bare
        return text

    lines = [ln.strip() for ln in text.splitlines()]
    body = [ln for ln in lines if ln and not _COMMENT.match(ln)]
    for line in body:
        if _plausible(line) is not None:
            return line
    # Some exports wrap the code across several lines.
    joined = "".join(body)
    if joined and _plausible(joined) is not None:
        return joined

    if body:
        raise ValueError("у вставленому тексті немає деккоду")
    raise ValueError("тут лише коментарі — бракує самого деккоду")


def deck_name(text):
    """The `### Name` title a paste carries, if it has one."""
    for line in (text or "").splitlines():
        line = line.strip()
        if line.startswith("###"):
            name = line.lstrip("#").strip()
            if name:
                return name
    return None


def decode(code):
    """Returns dict: format, heroes[dbf], cards[(dbf, count)],
    sideboards[(dbf, count, owner_dbf)].

    Accepts a bare deckstring or a pasted block around one.
    """
    return _parse(base64.b64decode(extract(code)))


def _parse(data):
    r = _Reader(data)
    if r.varint() != 0:
        raise ValueError("bad deckstring header")
    version = r.varint()
    fmt = r.varint()
    heroes = [r.varint() for _ in range(r.varint())]
    cards = []
    for _ in range(r.varint()):
        cards.append((r.varint(), 1))
    for _ in range(r.varint()):
        cards.append((r.varint(), 2))
    for _ in range(r.varint()):
        dbf = r.varint()
        cards.append((dbf, r.varint()))
    sideboards = []
    if not r.eof:
        has_sb = r.varint()
        if has_sb == 1:
            for _ in range(r.varint()):
                sideboards.append((r.varint(), 1, r.varint()))
            for _ in range(r.varint()):
                sideboards.append((r.varint(), 2, r.varint()))
            for _ in range(r.varint()):
                dbf = r.varint()
                n = r.varint()
                sideboards.append((dbf, n, r.varint()))
    return {"format": fmt, "version": version, "heroes": heroes,
            "cards": cards, "sideboards": sideboards}


def _varint(n):
    out = b""
    while True:
        b7 = n & 0x7F
        n >>= 7
        out += bytes([b7 | (0x80 if n else 0)])
        if not n:
            return out


def encode(hero_dbf, cards, fmt=2):
    """cards: list of (dbf, count). Returns deckstring."""
    ones = sorted(d for d, n in cards if n == 1)
    twos = sorted(d for d, n in cards if n == 2)
    more = [(d, n) for d, n in cards if n > 2]
    data = _varint(0) + _varint(1) + _varint(fmt)
    data += _varint(1) + _varint(hero_dbf)
    data += _varint(len(ones)) + b"".join(_varint(x) for x in ones)
    data += _varint(len(twos)) + b"".join(_varint(x) for x in twos)
    data += _varint(len(more))
    for d, n in more:
        data += _varint(d) + _varint(n)
    data += _varint(0)
    return base64.b64encode(data).decode()
