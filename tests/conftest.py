"""Shared fixtures. `hs2` card data is built once per session (~1 s)."""
import os
import sys
import tempfile

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

# `app` and `advisor` resolve their data directory at import time, and
# a test run must never land in the real one: it would read the
# developer's own games and write a cache into their profile.
# One stable directory rather than mkdtemp, which would leave a new
# one behind on every run.
os.environ.setdefault(
    "TAVERNLAB_HOME",
    os.path.join(tempfile.gettempdir(), "tavernlab-tests"))


@pytest.fixture(scope="session", autouse=True)
def carddb():
    from hs2 import carddata
    if not carddata.DEFS:
        carddata.build_defs()
    return carddata


@pytest.fixture(scope="session")
def metas(carddb):
    from hs2 import decks
    return decks.load_meta()


def build_deck(name, cls, cards, archetype="midrange", pad="Wisp",
               size=30):
    """A synthetic 30-card deck. `cards` is a list of names, in *draw
    order from the top* — i.e. the tail of `Player.deck` is drawn first."""
    from hs2 import carddata
    from hs2.decks import Deck
    ids = []
    for n in cards:
        ids.append(carddata.get_def(n).id)
    pad_id = carddata.get_def(pad).id
    while len(ids) < size:
        ids.insert(0, pad_id)
    return Deck(name, cls, archetype, ids)


def new_game(deck_a, deck_b, seed=1, style="midrange"):
    from hs2.ai import Agent
    from hs2.engine import Game
    return Game(deck_a, deck_b, seed=seed,
                agents=[Agent(style), Agent(style)])


def place(game, p, name, atk=None, hp=None, damage=0, taunt=None,
          divine_shield=None, ready=True):
    """Put a minion on `p`'s board, registered and (by default) able to
    attack this turn."""
    from hs2 import carddata
    from hs2.engine import Minion
    m = Minion(carddata.get_def(name), p)
    if atk is not None:
        m.atk_base = atk
    if hp is not None:
        m.hp_base = hp
    m.damage = damage
    if taunt is not None:
        m.taunt = taunt
    if divine_shield is not None:
        m.divine_shield = divine_shield
    if ready:
        m.just_summoned = False
        m.attacks_done = 0
    p.board.append(game.reg(m))
    return m


def give(game, p, name, n=1):
    """Add `n` copies of a card to `p`'s hand, registered."""
    from hs2 import carddata
    out = []
    for _ in range(n):
        inst = game.reg(carddata.make_inst_by_name(name))
        p.hand.append(inst)
        out.append(inst)
    return out


def bare_game(cls_a="MAGE", cls_b="PRIEST", seed=1, turn=6):
    """A started game with both boards and hands emptied — a blank slate
    for pinning a solver on one exact position."""
    g = new_game(build_deck("a", cls_a, []), build_deck("b", cls_b, []),
                 seed=seed)
    g.start(first=0)
    g.turn = turn
    g.current = 0
    for p in g.players:
        p.board.clear()
        p.hand.clear()
        p.hero_attacks = 0
        p.mana = p.crystals = 10
    return g
