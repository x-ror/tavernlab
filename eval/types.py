"""What the log actually reveals — **not** `hs2.Game`.

`VisibleState` is the reconstruction target: hero stats, mana, boards,
our hand, the opponent's hand as a count of face-down cards.  It is
deliberately independent of `hs2`, so a game full of unimplemented cards
still reconstructs and still gets a review (at `search_ok=0`).

Two flags, and they are not the same thing (design §4.2):

* `lethal_ok` — a *stats overlay* is faithful enough to run
  `hs2.lethal.find_lethal`.
* `search_ok` — the whole trigger graph is reconstructed.  **Always False
  in MVP.**  Nothing may publish a skill label while it is False.
"""
from dataclasses import asdict, dataclass, field
from typing import Literal, Optional

Zone = Literal["DECK", "HAND", "PLAY", "GRAVEYARD", "SETASIDE", "SECRET",
               "REMOVEDFROMGAME"]

SIDES = ("us", "them")


@dataclass
class EntityView:
    eid: int                    # the log's entity id; Hs2Mapper reuses it
    card_id: Optional[str] = None      # None while the card is hidden
    name: Optional[str] = None
    controller: int = 0                # 1 | 2
    zone: str = "SETASIDE"
    zone_pos: int = 0
    atk: Optional[int] = None
    health: Optional[int] = None
    damage: int = 0
    cost: Optional[int] = None
    tags: dict = field(default_factory=dict)   # TAUNT, DIVINE_SHIELD, ...

    @property
    def hp_left(self):
        if self.health is None:
            return None
        return self.health - self.damage

    def has(self, tag):
        return bool(self.tags.get(tag))

    def to_dict(self):
        return {k: v for k, v in asdict(self).items()
                if v not in (None, 0, {}, "")}

    @staticmethod
    def from_dict(d):
        e = EntityView(eid=d["eid"])
        for k, v in d.items():
            setattr(e, k, v)
        return e


@dataclass
class VisibleState:
    """`turn` is the GameEntity TURN tag, which **is** `hs2.Game.turn`:
    both increment once per player turn.  Do not hand it to
    `winprob.winprob_raw(my_turn=…)` — that helper doubles its argument.
    Use `wp_from_visible`."""
    turn: int = 0
    current_player: int = 1
    us: int = 1                        # our PlayerID (1|2)
    seq: int = 0                       # event seq this state is after
    mana: dict = field(default_factory=dict)    # pid -> crystals/used/temp
    heroes: dict = field(default_factory=dict)  # pid -> hp/armor/atk/...
    weapons: dict = field(default_factory=dict)
    # pid -> {eid, card_id, name, cost, exhausted}. Separate from
    # `heroes` because a hero power can be *replaced* mid-game (Justicar,
    # hero cards), so the class default is not a safe stand-in.
    hero_powers: dict = field(default_factory=dict)
    boards: dict = field(default_factory=dict)  # pid -> [EntityView]
    hands: dict = field(default_factory=dict)   # pid -> [EntityView]
    secrets: dict = field(default_factory=dict)
    deck_counts: dict = field(default_factory=dict)
    corpses: dict = field(default_factory=dict)
    quest: dict = field(default_factory=dict)
    implemented_gap: list = field(default_factory=list)
    lethal_ok: bool = False
    search_ok: bool = False            # MVP: always False

    # ------------------------------------------------------------ helpers
    @property
    def them(self):
        return 2 if self.us == 1 else 1

    def board(self, pid):
        return self.boards.get(pid, [])

    def minions(self, pid):
        return [e for e in self.board(pid)
                if e.tags.get("CARDTYPE") == "MINION"]

    def hand(self, pid):
        return self.hands.get(pid, [])

    def hero(self, pid):
        return self.heroes.get(pid, {})

    def hp_total(self, pid):
        h = self.hero(pid)
        return h.get("hp", 0) + h.get("armor", 0)

    def mana_left(self, pid):
        m = self.mana.get(pid, {})
        return max(0, m.get("crystals", 0) - m.get("used", 0)
                   - m.get("overload", 0) + m.get("temp", 0))

    def is_our_turn(self):
        return self.current_player == self.us

    def to_dict(self):
        d = asdict(self)
        d["boards"] = {p: [e.to_dict() for e in v]
                       for p, v in self.boards.items()}
        d["hands"] = {p: [e.to_dict() for e in v]
                      for p, v in self.hands.items()}
        return d

    @staticmethod
    def from_dict(d):
        vs = VisibleState()
        for k, v in d.items():
            if k in ("boards", "hands"):
                v = {int(p): [EntityView.from_dict(x) for x in lst]
                     for p, lst in v.items()}
            elif k in ("mana", "heroes", "weapons", "hero_powers",
                       "secrets", "deck_counts", "corpses", "quest"):
                v = {int(p): x for p, x in v.items()}
            setattr(vs, k, v)
        return vs


@dataclass
class Explanation:
    """Structured, template-rendered. Never an LLM (design §3.4)."""
    what: str = ""
    why_good: list = field(default_factory=list)
    why_bad: list = field(default_factory=list)
    better: list = field(default_factory=list)   # alternative actions
    tags: list = field(default_factory=list)
    strategic: list = field(default_factory=list)
    caveats: list = field(default_factory=list)

    def to_dict(self):
        return asdict(self)
