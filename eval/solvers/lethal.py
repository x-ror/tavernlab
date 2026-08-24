"""Missed-lethal detection — the MVP's only skill label.

Publish rule (design §3.3): the label goes out when `lethal_ok` holds
**and** `find_lethal` returns a plan on the stats overlay **and** the
player did not actually put the opponent to 0 that turn.

The asymmetry that makes this safe: the overlay only ever contains cards
we can model, so an unmodelled card can *add* damage but never invent it.
A lethal we find is therefore real; a lethal we miss is merely unknown.
That is why a positive result is gated on `lethal_ok` alone, while the
quiet "you had no lethal" reassurance additionally needs `hand_complete`.
"""
from eval.mapper import build_overlay

# Bound the extra ply: 20 ms per decision point (design §4.5).
DEEP_TURN_LIMIT = 40


class LethalFinding:
    __slots__ = ("available", "plan", "approx", "lethal_ok",
                 "hand_complete", "via_play", "taken", "unimplemented",
                 "reasons")

    def __init__(self):
        self.available = False
        self.plan = None            # human-readable line
        self.approx = False
        self.lethal_ok = False
        self.hand_complete = False
        self.via_play = None
        self.taken = False
        self.unimplemented = []
        self.reasons = []

    @property
    def missed(self):
        """The publishable condition, and nothing looser."""
        return bool(self.available and self.lethal_ok and not self.taken)

    def to_dict(self):
        return {"available": self.available, "plan": self.plan,
                "approx": self.approx, "lethal_ok": self.lethal_ok,
                "hand_complete": self.hand_complete,
                "via_play": self.via_play, "taken": self.taken,
                "missed": self.missed,
                "unimplemented": list(self.unimplemented),
                "reasons": list(self.reasons)}


def detect(vs, us_cls=None, them_cls=None, us_pid=None, taken=False,
           deep=True):
    """Was there lethal on the board in `vs`?

    `taken` says the player actually finished the game on this turn — the
    caller knows that from the log (opponent PLAYSTATE went LOST), and it
    turns a "missed lethal" into a quiet "played lethal".
    """
    from hs2.lethal import find_lethal

    out = LethalFinding()
    out.taken = bool(taken)
    us_pid = vs.us if us_pid is None else us_pid
    them_pid = 2 if us_pid == 1 else 1
    if vs.hp_total(them_pid) <= 0:
        out.reasons.append("opponent already at 0")
        return out

    ov = build_overlay(vs, us_cls=us_cls, them_cls=them_cls, us_pid=us_pid)
    out.lethal_ok = ov.lethal_ok
    out.hand_complete = ov.hand_complete
    out.unimplemented = ov.unimplemented
    out.reasons.extend(ov.reasons)
    if not ov.lethal_ok:
        return out

    use_deep = deep and (vs.turn or 0) <= DEEP_TURN_LIMIT
    try:
        plan = find_lethal(ov.game, ov.us, deep=use_deep)
    except Exception as exc:                      # never fail a review
        out.reasons.append(f"solver error: {type(exc).__name__}: {exc}")
        return out
    if plan is None:
        return out

    out.available = True
    out.plan = plan.describe() if hasattr(plan, "describe") else str(plan)
    out.approx = bool(getattr(plan, "approx", False))
    out.via_play = getattr(plan, "via_play", None)
    return out
