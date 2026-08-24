"""Which seqs are worth a snapshot, and the states that go with them.

The size budget (design §2.5) is the whole argument for this module: a
ranked game is 2k-8k events, and a `VisibleState` is 8-15 KB of JSON.
Snapshotting every `TAG_CHANGE` would cost tens of megabytes per game
and say nothing extra — the state between two bookkeeping tags is not a
state anybody decided anything in.  So we snapshot **decision points and
turn boundaries only**: 40-120 rows per game.

A decision point is the moment *before* a player commits to something:

* a top-level `BLOCK_START` of type `PLAY` (a card, a hero power or a
  location — the client logs all three as PLAY) or `ATTACK`;
* a top-level `POWER` block the acting player's own hero power or
  location opened (some clients log an activation that way);
* a `SEND_CHOICES` — mulligan or discover;
* every `TAG_CHANGE tag=TURN` **on the GameEntity**.  A player entity
  carries its own TURN tag counting only its own turns; mixing the two
  would double every turn boundary.

Snapshots are taken *before* the event, because that is the state the
player was looking at when they chose.
"""
from .visible import Reconstructor, card_name

# `SEND_CHOICES` choice types -> decision kind.
_CHOICE_KINDS = {"MULLIGAN": "mulligan", "GENERAL": "discover"}

# CARDTYPE of the acting entity -> kind, for blocks the client logs as
# PLAY / POWER regardless of what was actually activated.
_ACTOR_KINDS = {"HERO_POWER": "hero_power", "LOCATION": "location"}


def _side(rec, pid):
    if pid is None:
        return "us" if rec.current_player == rec.us else "them"
    return "us" if pid == rec.us else "them"


def _point(seq, turn, side, kind, entity_id=None, target_id=None,
           card_id=None, choices=None):
    # `name` is not part of the contract but costs nothing and keeps
    # `eval.review`'s rendered explanations from printing raw card ids.
    return {"seq": seq, "turn": turn, "side": side, "kind": kind,
            "entity_id": entity_id, "target_id": target_id,
            "card_id": card_id, "name": card_name(card_id),
            "choices": choices or []}


def _block_point(rec, ev, seq):
    """Top-level `BLOCK_START` -> a decision point, or None."""
    bt = ev.get("block_type")
    if bt not in ("PLAY", "ATTACK", "POWER"):
        return None
    eid = ev.get("entity_id")
    ent = rec.entities.get(eid)
    ctype = ent.cardtype() if ent is not None else None
    pid = ent.controller() if ent is not None else None
    if bt == "ATTACK":
        kind = "attack"
    elif bt == "PLAY":
        kind = _ACTOR_KINDS.get(ctype, "play")
    else:
        # A POWER block is usually a trigger resolving.  Only count it
        # when it is the acting player's own hero power / location —
        # anything else is not a decision anybody made here.
        kind = _ACTOR_KINDS.get(ctype)
        if kind is None or (pid and pid != rec.current_player):
            return None
    return _point(seq, rec.turn, _side(rec, pid or None), kind,
                  entity_id=eid, target_id=ev.get("target_id"),
                  card_id=(ent.card_id if ent is not None else None))


def _choice_point(rec, ev, seq):
    """`SEND_CHOICES` -> mulligan / discover.

    `choices` carries the whole offer, each entry flagged `picked`, so a
    later solver can grade both what was taken and what was passed on.
    """
    kind = _CHOICE_KINDS.get(str(ev.get("choice_type")), "discover")
    picked = [c for c in (ev.get("choices") or []) if c is not None]
    offer = rec.choices.get(ev.get("id"))
    pid = offer.get("player_id") if offer else None
    eids = [c for c in (offer.get("choices") or []) if c is not None] \
        if offer else list(picked)
    for c in picked:
        if c not in eids:
            eids.append(c)
    choices = []
    for c in eids:
        ent = rec.entities.get(c)
        choices.append({"eid": c,
                        "card_id": ent.card_id if ent is not None else None,
                        "picked": c in picked})
    first = rec.entities.get(picked[0]) if picked else None
    return _point(seq, rec.turn, _side(rec, pid), kind,
                  entity_id=picked[0] if picked else None,
                  card_id=first.card_id if first is not None else None,
                  choices=choices)


def _scan(events, us_pid=1, want_states=False):
    """One pass: decision points, and optionally the state before each.

    The reconstructor is fed *after* the point is built, so the state a
    point is paired with is the state the player saw.
    """
    rec = Reconstructor(us_pid)
    points, states = [], []
    for seq, ev in enumerate(events):
        if not isinstance(ev, dict):
            continue
        kind = ev.get("type")
        pt = None
        if kind == "BLOCK_START":
            if rec.depth == 0:
                pt = _block_point(rec, ev, seq)
        elif kind == "SEND_CHOICES":
            pt = _choice_point(rec, ev, seq)
        elif kind == "TAG_CHANGE" and ev.get("tag") == "TURN" \
                and ev.get("entity_id") == rec.game_eid \
                and rec.game_eid is not None:
            # `turn` is the turn being *entered*; CURRENT_PLAYER has
            # already flipped by the time the GameEntity TURN lands, so
            # `rec.current_player` is the player about to act.
            pt = _point(seq, _turn_value(ev, rec), _side(rec, None),
                        "turn_start", entity_id=ev.get("entity_id"))
        if pt is not None:
            if want_states:
                states.append(rec.state(seq))
            points.append(pt)
        rec.apply(ev, seq)
    return states, points


def _turn_value(ev, rec):
    v = ev.get("value")
    return v if isinstance(v, int) else rec.turn


def decision_points(events):
    """The seqs worth snapshotting, in order."""
    return _scan(events, want_states=False)[1]


def build_snapshots(events, us_pid=1):
    """`(states, points)` — the state *before* each decision point."""
    return _scan(events, us_pid, want_states=True)


def snapshot_rows(states, points=None):
    """`VisibleState`s -> `store.add_snapshots` rows.

    `wp` / `wp_source` stay None: the win-probability series is PR 10's,
    and a hatched `logistic_v1` number written here would be a number
    nobody computed.  `search_ok` is 0 by MVP invariant (design §2.6).
    """
    rows = []
    for vs in states:
        rows.append({
            "event_seq": vs.seq,
            "visible": vs.to_dict(),
            "lethal_ok": 1 if vs.lethal_ok else 0,
            "search_ok": 1 if vs.search_ok else 0,
            "unimplemented": list(vs.implemented_gap),
            "wp": None,
            "wp_source": None,
        })
    return rows
