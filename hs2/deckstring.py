"""Minimal Hearthstone deckstring decoder (no external dependencies).

Format: base64 of [0x00, version, format, heroes, 1x-cards, 2x-cards,
nx-cards, (optional sideboard section)]. All numbers are varints.
"""
import base64


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


def decode(code):
    """Returns dict: format, heroes[dbf], cards[(dbf, count)],
    sideboards[(dbf, count, owner_dbf)]."""
    data = base64.b64decode(code.strip())
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
