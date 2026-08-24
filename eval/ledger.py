"""L0: deterministic turn accounting.

No search, no model, no clone — just arithmetic on what the log shows, so
it is always publishable and always correct.  These are **notes**, never
skill glyphs: "you ended on 2 mana with a 2-drop in hand" is a fact; "that
was a mistake" is a claim this layer is not entitled to make.

Budget: <1 ms per turn (design §3.2).
"""
from eval.types import Explanation

# What the log calls "this minion cannot attack right now".
_BLOCKED = ("FROZEN", "CANT_ATTACK", "DORMANT")


class TurnLedger:
    __slots__ = ("turn", "side", "mana_left", "crystals", "unused_attacks",
                 "unused_attackers", "affordable_unplayed",
                 "hero_power_skipped", "hero_attack_unused", "lethal",
                 "notes")

    def __init__(self, turn, side):
        self.turn = turn
        self.side = side
        self.mana_left = 0
        self.crystals = 0
        self.unused_attacks = 0
        self.unused_attackers = []
        self.affordable_unplayed = []
        self.hero_power_skipped = False
        self.hero_attack_unused = False
        self.lethal = False
        self.notes = []

    def to_dict(self):
        return {"turn": self.turn, "side": self.side,
                "mana_left": self.mana_left, "crystals": self.crystals,
                "unused_attacks": self.unused_attacks,
                "unused_attackers": list(self.unused_attackers),
                "affordable_unplayed": list(self.affordable_unplayed),
                "hero_power_skipped": self.hero_power_skipped,
                "hero_attack_unused": self.hero_attack_unused,
                "lethal": self.lethal,
                "notes": [n.to_dict() if hasattr(n, "to_dict") else n
                          for n in self.notes]}


def can_still_attack(view):
    """Would this minion have been allowed one more swing?"""
    tags = view.tags or {}
    if tags.get("CARDTYPE") not in (None, "MINION"):
        return False
    if not (view.atk or 0):
        return False
    if any(tags.get(t) for t in _BLOCKED):
        return False
    if (view.hp_left or 0) <= 0:
        return False
    done = int(tags.get("NUM_ATTACKS_THIS_TURN") or 0)
    allowed = 2 if tags.get("WINDFURY") else 1
    if done >= allowed:
        return False
    # EXHAUSTED with no swings taken is summoning sickness, unless the
    # minion has Charge (Rush still cannot go face, but it can trade).
    if tags.get("EXHAUSTED") and done == 0 and not (
            tags.get("CHARGE") or tags.get("RUSH")):
        return False
    return True


def build(vs, us_pid=None, side="us", hero_power_used=None,
          hero_attacked=None):
    """Account for the turn that just ended, from its final VisibleState."""
    pid = (vs.us if us_pid is None else us_pid)
    if side == "them":
        pid = 2 if pid == 1 else 1
    led = TurnLedger(int(vs.turn or 0), side)

    mana = vs.mana.get(pid) or {}
    led.crystals = int(mana.get("crystals") or 0)
    led.mana_left = vs.mana_left(pid)

    for m in vs.board(pid):
        if can_still_attack(m):
            allowed = 2 if (m.tags or {}).get("WINDFURY") else 1
            done = int((m.tags or {}).get("NUM_ATTACKS_THIS_TURN") or 0)
            led.unused_attacks += allowed - done
            led.unused_attackers.append(m.name or m.card_id or f"#{m.eid}")

    hero = vs.hero(pid) or {}
    if hero_attacked is None:
        hero_attacked = bool(hero.get("attacks"))
    led.hero_attack_unused = bool(hero.get("atk")) and not hero_attacked
    if hero_power_used is not None:
        led.hero_power_skipped = not hero_power_used

    for card in vs.hand(pid):
        cost = card.cost
        if cost is None or not (card.name or card.card_id):
            continue          # face-down: we cannot know what it cost
        if cost <= led.mana_left:
            led.affordable_unplayed.append(
                {"card": card.name or card.card_id, "cost": cost})

    led.notes = notes(led)
    return led


def notes(led):
    """Template-rendered, keyed for i18n. Never an LLM (design §3.4)."""
    out = []
    if led.mana_left > 0 and led.affordable_unplayed:
        cards = ", ".join(c["card"] for c in led.affordable_unplayed[:3])
        out.append(Explanation(
            what=f"Ended turn {led.turn} with {led.mana_left} mana unspent",
            why_bad=[f"{cards} was affordable"],
            tags=["mana_waste", "note"],
            caveats=["Holding mana is sometimes correct (secrets, "
                     "combo, play-around); this is an observation, "
                     "not a verdict."]))
    elif led.mana_left >= 2:
        out.append(Explanation(
            what=f"Ended turn {led.turn} with {led.mana_left} mana unspent",
            tags=["mana_waste", "note"]))
    if led.unused_attacks:
        who = ", ".join(led.unused_attackers[:3])
        out.append(Explanation(
            what=f"{led.unused_attacks} attack(s) unused on turn "
                 f"{led.turn}",
            why_bad=[f"{who} could still have swung"],
            tags=["unused_attack", "note"]))
    if led.hero_attack_unused:
        out.append(Explanation(
            what=f"Hero attack unused on turn {led.turn}",
            tags=["unused_attack", "note"]))
    if led.hero_power_skipped and led.mana_left >= 2:
        out.append(Explanation(
            what=f"Hero power unused on turn {led.turn} "
                 f"({led.mana_left} mana left)",
            tags=["hero_power", "note"]))
    return out
