"""Serializable pending effects (PR 2).

Player-level pending effects used to be **closures** captured over the live
`Player` / `CardInst` objects (`Player.listen(event, fn)` /
`Player.at_turn_start(fn)`).  A cloned `Game` that copied those lists kept
firing them against the *original* objects, so search and lethal-from-clone
were unsound.

Everything player-level is now a dataclass holding a `handler_id` (a key in
`HANDLERS`, populated by `hs2.impls` via `@handler`), a `source_eid`, and a
JSON-serializable `args` dict.  Clone copies the dataclasses and resolves
`source_eid` through the *clone's* entity map.

`Minion.triggers` are NOT affected: they come from `CardDef` and already take
`(g, p, m, *args)`, so sharing the function while copying the minion is safe.
"""
from dataclasses import dataclass, field

# handler_id -> callable
#   listeners:  fn(game, owner, source, *event_args, **args)
#   turn start: fn(game, owner, source, **args)
HANDLERS = {}


def handler(name):
    """Register a named, clone-safe effect handler."""
    def deco(fn):
        HANDLERS[name] = fn
        return fn
    return deco


@dataclass
class PendingListen:
    event: str
    handler_id: str          # key in HANDLERS, never a lambda
    source_eid: int          # entity that registered it (player eid 1|2 ok)
    expiry_turn: int = None
    args: dict = field(default_factory=dict)   # JSON-serializable only

    def copy(self):
        return PendingListen(self.event, self.handler_id, self.source_eid,
                             self.expiry_turn, dict(self.args))

    def to_dict(self):
        return {"event": self.event, "handler_id": self.handler_id,
                "source_eid": self.source_eid,
                "expiry_turn": self.expiry_turn, "args": dict(self.args)}


@dataclass
class PendingTurnStart:
    handler_id: str
    source_eid: int
    turns_left: int = 1
    repeat: bool = False
    args: dict = field(default_factory=dict)

    def copy(self):
        return PendingTurnStart(self.handler_id, self.source_eid,
                                self.turns_left, self.repeat,
                                dict(self.args))

    def to_dict(self):
        return {"handler_id": self.handler_id,
                "source_eid": self.source_eid,
                "turns_left": self.turns_left, "repeat": self.repeat,
                "args": dict(self.args)}
