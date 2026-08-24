"""The canonical event catalog (closed) and the packet -> event encoder.

Two rules the rest of the pipeline depends on:

1. **Depth-first.**  hslog nests `BLOCK_START`/`BLOCK_END` as a `Block` whose
   children live in `block.packets`.  A flat `for p in tree.packets` drops
   every TAG_CHANGE and META_DATA inside a PLAY / ATTACK / TRIGGER block —
   the same class of loss as `EntityTreeExporter`, which is why that
   exporter is not the importer.
2. **One logger stream per file.**  Power.log carries both
   `GameState.DebugPrintPower()` and `PowerTaskList.DebugPrintPower()`.
   Parsing both doubles the tree.

The live tailer (v1) must emit these same `type` strings so `eval/visible.py`
has exactly one reducer to maintain.
"""
from hearthstone.enums import BlockType, GameTag, MetaDataType

CREATE_GAME = "CREATE_GAME"
FULL_ENTITY = "FULL_ENTITY"
SHOW_ENTITY = "SHOW_ENTITY"
HIDE_ENTITY = "HIDE_ENTITY"
CHANGE_ENTITY = "CHANGE_ENTITY"
TAG_CHANGE = "TAG_CHANGE"
BLOCK_START = "BLOCK_START"
BLOCK_END = "BLOCK_END"
META_DATA = "META_DATA"
CHOICES = "CHOICES"
CHOSEN_ENTITIES = "CHOSEN_ENTITIES"
SEND_CHOICES = "SEND_CHOICES"
OPTIONS = "OPTIONS"
SEND_OPTION = "SEND_OPTION"
RESET_GAME = "RESET_GAME"
SUB_SPELL = "SUB_SPELL"
SHUFFLE_DECK = "SHUFFLE_DECK"
ZONE_MOVE = "ZONE_MOVE"
OTHER = "OTHER"

EVENT_TYPES = frozenset({
    CREATE_GAME, FULL_ENTITY, SHOW_ENTITY, HIDE_ENTITY, CHANGE_ENTITY,
    TAG_CHANGE, BLOCK_START, BLOCK_END, META_DATA, CHOICES,
    CHOSEN_ENTITIES, SEND_CHOICES, OPTIONS, SEND_OPTION, RESET_GAME,
    SUB_SPELL, SHUFFLE_DECK, ZONE_MOVE, OTHER,
})

# hslog class name -> canonical type
_BY_CLASS = {
    "CreateGame": CREATE_GAME,
    "FullEntity": FULL_ENTITY,
    "ShowEntity": SHOW_ENTITY,
    "HideEntity": HIDE_ENTITY,
    "ChangeEntity": CHANGE_ENTITY,
    "TagChange": TAG_CHANGE,
    "MetaData": META_DATA,
    "Choices": CHOICES,
    "ChosenEntities": CHOSEN_ENTITIES,
    "SendChoices": SEND_CHOICES,
    "Options": OPTIONS,
    "SendOption": SEND_OPTION,
    "ResetGame": RESET_GAME,
    "SubSpell": SUB_SPELL,
    "ShuffleDeck": SHUFFLE_DECK,
}


def tag_name(tag):
    """`GameTag.ZONE` -> "ZONE"; unknown numeric tags keep their number."""
    try:
        return GameTag(int(tag)).name
    except (ValueError, TypeError):
        return str(tag)


def plain(v):
    """JSON-safe value: enums become their name, everything else int/str.

    hslog hands some numeric fields back as strings (`effectindex`,
    `MetaData.data`); they are coerced so the reducer never has to guess.
    """
    if v is None:
        return None
    if isinstance(v, bool):
        return int(v)
    name = getattr(v, "name", None)
    if name is not None and not isinstance(v, (str, bytes)):
        return name
    if isinstance(v, int):
        return v
    if isinstance(v, str):
        s = v.strip()
        if s.lstrip("-").isdigit():
            return int(s)
        return v
    return str(v)


def entity_id(e):
    """hslog leaves either an int or a `PlayerReference` on `.entity`."""
    if e is None:
        return None
    if isinstance(e, int):
        return e
    return getattr(e, "entity_id", None)


def _tags(pairs):
    return {tag_name(t): plain(v) for t, v in (pairs or [])}


def canon_block_start(b):
    try:
        bt = BlockType(int(b.type)).name
    except (ValueError, TypeError):
        bt = str(b.type)
    return {"type": BLOCK_START, "block_type": bt,
            "entity_id": entity_id(b.entity),
            "effect_id": b.effectid or None,
            "effect_index": plain(b.effectindex),
            "target_id": entity_id(b.target) or None,
            "trigger_keyword": plain(b.trigger_keyword)}


def canon_block_end(b):
    try:
        bt = BlockType(int(b.type)).name
    except (ValueError, TypeError):
        bt = str(b.type)
    return {"type": BLOCK_END, "block_type": bt,
            "entity_id": entity_id(b.entity)}


def canonicalize(p):
    """One non-Block packet -> one canonical event dict.

    Unknown packet types become `OTHER` with a best-effort dump: the
    importer must never crash on a packet hslog added after we pinned it.
    """
    cls = type(p).__name__
    kind = _BY_CLASS.get(cls, OTHER)
    if kind == CREATE_GAME:
        return {"type": CREATE_GAME, "entity_id": entity_id(p.entity),
                "tags": _tags(getattr(p, "tags", None)),
                "players": [{"entity_id": entity_id(pl.entity),
                             "player_id": pl.player_id,
                             "name": pl.name,
                             "tags": _tags(getattr(pl, "tags", None))}
                            for pl in getattr(p, "players", [])]}
    if kind in (FULL_ENTITY, SHOW_ENTITY, CHANGE_ENTITY):
        return {"type": kind, "entity_id": entity_id(p.entity),
                "card_id": p.card_id or None,
                "tags": _tags(getattr(p, "tags", None))}
    if kind == HIDE_ENTITY:
        return {"type": kind, "entity_id": entity_id(p.entity),
                "zone": plain(p.zone)}
    if kind == TAG_CHANGE:
        return {"type": kind, "entity_id": entity_id(p.entity),
                "tag": tag_name(p.tag), "value": plain(p.value)}
    if kind == META_DATA:
        try:
            meta = MetaDataType(int(p.meta)).name
        except (ValueError, TypeError):
            meta = str(p.meta)
        data = p.data
        return {"type": kind, "meta": meta,
                "data": [plain(x) for x in data] if isinstance(
                    data, (list, tuple)) else [plain(data)],
                "info": [entity_id(x) for x in (p.info or [])],
                "count": p.count}
    if kind == CHOICES:
        return {"type": kind, "id": p.id,
                "player_id": getattr(p.entity, "player_id", None),
                "entity_id": entity_id(p.entity),
                "choice_type": plain(p.type), "min": p.min, "max": p.max,
                "source": entity_id(p.source), "tasklist": p.tasklist,
                "choices": [entity_id(c) for c in (p.choices or [])]}
    if kind == CHOSEN_ENTITIES:
        return {"type": kind, "id": p.id,
                "player_id": getattr(p.entity, "player_id", None),
                "entity_id": entity_id(p.entity),
                "choices": [entity_id(c) for c in (p.choices or [])]}
    if kind == SEND_CHOICES:
        return {"type": kind, "id": p.id, "choice_type": plain(p.type),
                "choices": [entity_id(c) for c in (p.choices or [])]}
    if kind == OPTIONS:
        # Verbose `DebugPrintOptions` only. Stored now, consumed by the v1
        # options-packet legal set (PR 21).
        return {"type": kind, "id": p.id,
                "options": [{"index": o.id, "opt_type": plain(o.type),
                             "entity_id": entity_id(o.entity),
                             "error": plain(o.error)}
                            for o in (p.options or [])]}
    if kind == SEND_OPTION:
        return {"type": kind, "option": p.option, "suboption": p.suboption,
                "target": entity_id(p.target), "position": p.position}
    if kind == SUB_SPELL:
        return {"type": kind, "phase": "start",
                "prefab": p.spell_prefab_guid,
                "source": entity_id(p.source),
                "target_count": p.target_count,
                "targets": [entity_id(t) for t in (p.targets or [])]}
    if kind == SHUFFLE_DECK:
        return {"type": kind, "player_id": p.player_id}
    if kind == RESET_GAME:
        return {"type": kind}
    return {"type": OTHER, "packet": cls,
            "raw": {k: plain(v) for k, v in vars(p).items()
                    if k not in ("ts", "packets")}}


def ts_of(p):
    """Log timestamp as text. Not thinking time (animations, rope) — see
    design §3.5 "Decision-time": store it, do not grade it."""
    ts = getattr(p, "ts", None)
    return None if ts is None else str(ts)


def walk(packets, out):
    """Depth-first, the only correct traversal (design §2.5).

    `Block` is not the only container: hslog's `SubSpell` also carries a
    `.packets` list, and a spell's real work often happens in there —
    on one 40-turn fixture, 872 packets (659 TAG_CHANGEs, 93 FULL_ENTITYs
    and 9 HIDE_ENTITYs) hang off 50 sub-spells.  Dropping them left a
    bounced minion sitting on the board for the rest of the game and a
    hero whose HEALTH buff never arrived.  So the recursion tests for
    "has children", not for a class name.
    """
    for p in packets:
        kids = getattr(p, "packets", None)
        if type(p).__name__ == "Block":
            ev = canon_block_start(p)
            ev["ts"] = ts_of(p)
            out.append(ev)
            walk(kids or [], out)         # recurse — required
            ev = canon_block_end(p)
            ev["ts"] = ts_of(p)
            out.append(ev)
            continue
        ev = canonicalize(p)
        ev["ts"] = ts_of(p)
        out.append(ev)
        if ev["type"] == SUB_SPELL:
            # The end marker is emitted even for a childless sub-spell
            # (10 of 60 on one fixture). A consumer that brackets on the
            # pair would otherwise treat the whole rest of the stream as
            # nested inside the unclosed one.
            if kids:
                walk(kids, out)
            out.append({"type": SUB_SPELL, "phase": "end",
                        "prefab": ev.get("prefab"),
                        "source": ev.get("source"),
                        "ts": ev["ts"]})
        elif kids:
            walk(kids, out)
    return out
