"""Which labels we are allowed to publish, and why the rest are hidden.

The whole point of this module is that the gates live in **one** place and
are inspectable, so the UI can grey out "Blunder — coming when calibrated"
instead of quietly shipping a Chess.com glyph derived from a 9-weight
logistic model (design §3.1, §3.3).

MVP publishes exactly: `missed_lethal`, `played_lethal` (quiet), `note`.
Everything else returns a `hidden` verdict carrying the gate it failed.
"""
from dataclasses import dataclass, field

EVALUATOR_VERSION = "eval-0.1.0+hs2"

# ΔWP thresholds. Present so they are reviewable, NOT active: every one
# of them additionally requires search_ok ∧ actions_complete, which the
# MVP never sets. Recalibrate after 500 reviewed human games.
DELTA_WP_THRESHOLDS = {"blunder": -0.15, "mistake": -0.08,
                       "inaccuracy": -0.03}


@dataclass(frozen=True)
class LabelSpec:
    key: str
    phase: str                     # mvp | v1 | v2
    needs: tuple = ()              # gate names that must all be true
    note: str = ""


LABELS = (
    LabelSpec("missed_lethal", "mvp", ("lethal_ok", "lethal_available",
                                       "not_taken"),
              "Line exists that ends the game this turn."),
    LabelSpec("played_lethal", "mvp", ("lethal_taken",),
              "Positive acknowledgement; shown quietly."),
    LabelSpec("note", "mvp", (),
              "Ledger leak. Not a skill glyph."),
    LabelSpec("lucky", "v1", ("meta_data_parsed",),
              "Needs a META_DATA fixture going green first (PR 14)."),
    LabelSpec("unlucky", "v1", ("meta_data_parsed",), ""),
    LabelSpec("inaccuracy", "v1",
              ("search_ok", "actions_complete", "search_depth", "delta_wp"),
              "ΔWP <= -0.03 once the model is calibrated."),
    LabelSpec("mistake", "v1",
              ("search_ok", "actions_complete", "search_depth", "delta_wp"),
              "ΔWP <= -0.08."),
    LabelSpec("blunder", "v1",
              ("search_ok", "actions_complete", "search_depth", "delta_wp"),
              "ΔWP <= -0.15."),
    LabelSpec("best", "v2", ("search_ok", "actions_complete"),
              "Uniquely max among the complete legal set."),
    LabelSpec("brilliant", "v2", ("search_ok", "actions_complete"),
              "Never derivable from L0/L2."),
)

BY_KEY = {spec.key: spec for spec in LABELS}
MVP_LABELS = tuple(s.key for s in LABELS if s.phase == "mvp")
COMING_SOON = tuple(s.key for s in LABELS if s.phase != "mvp")


@dataclass
class Verdict:
    label: str = None
    conf: float = None
    reason: str = ""
    gates: dict = field(default_factory=dict)

    def to_dict(self):
        return {"label": self.label, "label_conf": self.conf,
                "label_reason": self.reason, "gates": dict(self.gates)}


def gates_for(*, lethal=None, actions_complete=False, search_ok=False,
              search_depth=0, delta_wp=None, meta_data_parsed=False):
    """Collect every gate one decision satisfies."""
    lethal = lethal or {}
    return {
        "lethal_ok": bool(lethal.get("lethal_ok")),
        "hand_complete": bool(lethal.get("hand_complete")),
        "lethal_available": bool(lethal.get("available")),
        "lethal_taken": bool(lethal.get("taken")),
        "not_taken": not lethal.get("taken"),
        # `classify` downgrades an approximate line to 0.75. Dropping the
        # flag here published a bounded taunt search at 0.95 and hid the
        # "not proven exact" warning with it.
        "approx": bool(lethal.get("approx")),
        "actions_complete": bool(actions_complete),
        "search_ok": bool(search_ok),
        "search_depth": int(search_depth or 0) >= 1,
        "delta_wp": delta_wp is not None,
        "meta_data_parsed": bool(meta_data_parsed),
    }


def classify(gates):
    """The single publish decision. Returns a `Verdict`.

    `label=None` is the normal outcome and is not a failure: it means no
    label passed its gates, so the UI shows the structured explanation and
    the ledger notes instead of a glyph.
    """
    if gates.get("lethal_taken"):
        return Verdict("played_lethal", 1.0,
                       "lethal executed", dict(gates))
    if all(gates.get(g) for g in BY_KEY["missed_lethal"].needs):
        conf = 0.75 if gates.get("approx") else 0.95
        return Verdict("missed_lethal", conf,
                       "lethal_ok and a line exists that was not taken",
                       dict(gates))
    return Verdict(None, None, hidden_reason(gates), dict(gates))


def hidden_reason(gates):
    """Say *why* nothing was published; the UI shows this verbatim."""
    if not gates.get("search_ok"):
        return ("search_ok=0; logistic ΔWP is not a ranking signal")
    if not gates.get("actions_complete"):
        return "actions_complete=0; the legal set is not provably complete"
    return "no label passed its gate"


def wp_caveat():
    """The sentence that must accompany every win-probability chart."""
    return ("WP is logistic, 8 board features + bias (9 weights), 66.7% "
            "on sim snapshots; hatched; not a play-ranking oracle; not "
            "calibrated on your games.")


def legend():
    """What the UI shows in its greyed 'coming when calibrated' legend."""
    return [{"key": s.key, "phase": s.phase, "needs": list(s.needs),
             "note": s.note, "available": s.phase == "mvp"}
            for s in LABELS]
