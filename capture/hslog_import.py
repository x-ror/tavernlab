"""Power.log -> canonical events, via the hslog Packet Tree.

Why not `hslog.export.EntityTreeExporter`: it hands back the *final* entity
tree, so a card that was played and then died shows only its last zone.  The
turn-by-turn board a review needs is in the packets, not the tree.

The importer is deliberately independent of `hs2`: a game whose cards are not
implemented still imports, reconstructs, and reviews at `search_ok=0`.
"""
import gzip
import hashlib
import io
import json
import os
import re
import time

from hearthstone.enums import CardClass, FormatType, GameType

from .events import (BLOCK_START, CREATE_GAME, FULL_ENTITY, SHOW_ENTITY,
                     TAG_CHANGE, walk)

# A Power.log with logging barely enabled is a few hundred bytes; a real
# game slice is 0.5-3 MB.  `live.py` uses the same guard.
MIN_POWER_BYTES = 20_000

# MVP import is always a finished hslog parse (design §2.2 generation rule);
# generation 0 is reserved for the v1 live tail.
PARSE_GENERATION = 1

_LINE_RE = re.compile(r"^D\s+[\d:.]+\s+(?P<logger>\w+)\.")
_CREATE_RE = re.compile(r"\)\s*-\s*CREATE_GAME\s*$")

# Zone.log: "... id=4 ... player=1] zone from  -> FRIENDLY DECK"
_ZONE_SIDE_RE = re.compile(r"player=(\d+)\].*?->\s+(FRIENDLY|OPPOSING)\s")

MODE_BY_GAME_TYPE = {
    GameType.GT_RANKED: "ranked",
    GameType.GT_CASUAL: "casual",
    GameType.GT_VS_FRIEND: "friendly",
    GameType.GT_ARENA: "arena",
    GameType.GT_BATTLEGROUNDS: "bg",
    GameType.GT_VS_AI: "vs_ai",
}


# ------------------------------------------------------------ line slicing
def choose_logger(lines):
    """PowerTaskList if it emitted anything, else GameState.

    Never both: each logger prints the *same* game, so parsing both doubles
    every packet.
    """
    seen = set()
    for ln in lines:
        m = _LINE_RE.match(ln)
        if m:
            seen.add(m.group("logger"))
    if "PowerTaskList" in seen:
        return "PowerTaskList"
    return "GameState"


def filter_logger(lines, logger):
    """Keep exactly one power stream, plus the GameState-only side channels.

    Power packets come from `logger`.  Game meta (`DebugPrintGame`), choices
    and options are **only ever** emitted by `GameState.*`, so those lines
    stay regardless — dropping them would cost us BuildNumber/GameType and
    every mulligan/discover decision.
    """
    out, meta, seen_create = [], [], False
    for ln in lines:
        m = _LINE_RE.match(ln)
        if not m:
            continue
        who = m.group("logger")
        if who == "GameState" and "DebugPrintGame()" in ln \
                and logger != "GameState":
            # Hoisted below: see the comment at the CREATE_GAME append.
            if not seen_create:
                meta.append(ln)
                continue
        if who == logger:
            out.append(ln)
            if not seen_create and _CREATE_RE.search(ln):
                seen_create = True
                # hslog drops every line before the CREATE_GAME *of the
                # stream it is watching*. GameState prints DebugPrintGame
                # (BuildNumber, GameType, and the PlayerID->PlayerName
                # binding) a few hundred lines before PowerTaskList's
                # CREATE_GAME, so watching PowerTaskList would silently
                # strand the name binding and hslog would then mis-guess
                # which battletag owns which player_id. Replaying those
                # lines here costs nothing: handle_game is order-free once
                # the game exists.
                out.extend(meta)
                meta = []
        elif who == "GameState" and "DebugPrintPower()" not in ln:
            out.append(ln)
    return out


def make_parser(logger):
    """`LogParser` pointed at `logger` for power packets.

    hslog hard-codes the `GameState` processor prefix on every handler.
    Only the *power* handler is repointed, and it must still answer
    `GameState.DebugPrintGame` because PowerTaskList never logs game meta.
    """
    from hslog import LogParser
    parser = LogParser()
    if logger != "GameState":
        ph = parser._power_handler
        ph._game_state_processor = logger
        base = ph.find_callback

        def find_callback(method, _ph=ph, _base=base):
            if method == "GameState.DebugPrintGame":
                return _ph.handle_game
            return _base(method)

        ph.find_callback = find_callback
    return parser


def split_games(lines, boundary="GameState"):
    """Split **raw** lines into per-game slices at `boundary`'s CREATE_GAME.

    Two reasons this runs before the logger filter, not after:

    * One `LogParser` per slice. hslog raises `InconsistentPlayerIdError`
      when a battletag changes player_id between games in one stream, which
      is exactly what a multi-game session does.
    * `GameState.DebugPrintGame` (BuildNumber, GameType, and the
      `PlayerID=n, PlayerName=…` binding) is printed a few hundred lines
      *after* GameState's CREATE_GAME but *before* PowerTaskList's. Cutting
      on the PowerTaskList line strands that binding in the previous slice,
      and hslog then guesses the name/player_id pairing and blows up.
    """
    out, cur = [], None
    for ln in lines:
        m = _LINE_RE.match(ln)
        is_start = (m and m.group("logger") == boundary
                    and _CREATE_RE.search(ln))
        if is_start:
            if cur:
                out.append(cur)
            cur = [ln]
        elif cur is not None:
            cur.append(ln)
    if cur:
        out.append(cur)
    return out


def boundary_logger(lines):
    """GameState prints CREATE_GAME first, so it is the natural cut."""
    for ln in lines:
        m = _LINE_RE.match(ln)
        if m and m.group("logger") == "GameState" and _CREATE_RE.search(ln):
            return "GameState"
    return choose_logger(lines)


def read_lines(path):
    """Read a Power.log. `.gz` is accepted so committed fixtures stay small
    (a real game slice is ~1.5 MB raw, ~85 KB gzipped)."""
    if str(path).endswith(".gz"):
        with gzip.open(path, "rt", encoding="utf-8", errors="replace") as fh:
            return fh.readlines()
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.readlines()


# ------------------------------------------------------------ packet parse
def parse_slice(lines, logger="GameState"):
    """One game slice -> hslog PacketTree (or None if it never opened)."""
    parser = make_parser(logger)
    parser.read(io.StringIO("".join(lines)))
    parser.flush()
    if not parser.games:
        return None, parser
    return parser.games[0], parser


def events_from_tree(tree):
    return walk(tree.packets, [])


# --------------------------------------------------------------- identity
def friendly_controller(zone_log_path):
    """Our controller id (1|2) from Zone.log FRIENDLY/OPPOSING.

    Battletag-free identity: the client always calls *our* side FRIENDLY.
    Returns None when Zone.log is absent — then the caller falls back to the
    battletag setting, and if that is empty too, to controller 1.
    """
    if not zone_log_path or not os.path.exists(zone_log_path):
        return None
    votes = {}
    try:
        with open(zone_log_path, encoding="utf-8", errors="replace") as fh:
            for ln in fh:
                m = _ZONE_SIDE_RE.search(ln)
                if m:
                    pid, side = int(m.group(1)), m.group(2)
                    if side == "FRIENDLY":
                        votes[pid] = votes.get(pid, 0) + 1
                    else:
                        votes.setdefault(pid, 0)
    except OSError:
        return None
    if not votes:
        return None
    best = max(votes, key=votes.get)
    return best if votes[best] else None


# ------------------------------------------------------------- summarising
class GameSummary:
    """Everything the `games` row needs, scanned off the canonical events."""

    __slots__ = ("players", "classes", "winner_pid", "turns", "mode",
                 "fmt", "first_pid", "hero_eid", "build")

    def __init__(self):
        self.players = {}        # player_id -> {"entity_id", "name"}
        self.classes = {}        # player_id -> class name
        self.winner_pid = None
        self.turns = 0
        self.mode = "unknown"
        self.fmt = None
        self.first_pid = None
        self.hero_eid = {}       # player_id -> hero entity id
        self.build = None

    def result_for(self, pid):
        if self.winner_pid is None:
            return "unknown"
        if self.winner_pid == 0:
            return "tie"
        return "win" if self.winner_pid == pid else "loss"


def summarize(events, game_meta=None):
    """Winner, classes, turn count — the PR 5 acceptance triple."""
    s = GameSummary()
    ctrl_of = {}            # entity_id -> controller
    card_of = {}            # entity_id -> card_id
    eid_to_pid = {}         # player entity id -> player_id
    lost = set()

    for ev in events:
        t = ev["type"]
        if t == CREATE_GAME:
            for pl in ev.get("players", []):
                pid = pl.get("player_id")
                if pid is None:
                    continue
                s.players[pid] = {"entity_id": pl.get("entity_id"),
                                  "name": pl.get("name")}
                eid_to_pid[pl.get("entity_id")] = pid
                tags = pl.get("tags") or {}
                if tags.get("HERO_ENTITY"):
                    s.hero_eid[pid] = tags["HERO_ENTITY"]
                if tags.get("FIRST_PLAYER"):
                    s.first_pid = pid
        elif t in (FULL_ENTITY, SHOW_ENTITY):
            eid = ev.get("entity_id")
            tags = ev.get("tags") or {}
            if ev.get("card_id"):
                card_of[eid] = ev["card_id"]
            if "CONTROLLER" in tags:
                ctrl_of[eid] = tags["CONTROLLER"]
            if tags.get("CARDTYPE") == "HERO" and "CONTROLLER" in tags:
                pid = tags["CONTROLLER"]
                s.hero_eid.setdefault(pid, eid)
                if tags.get("CLASS"):
                    s.classes[pid] = tags["CLASS"]
        elif t == TAG_CHANGE:
            eid, tag, val = ev["entity_id"], ev["tag"], ev["value"]
            if tag == "TURN" and isinstance(val, int):
                s.turns = max(s.turns, val)
            elif tag == "PLAYSTATE":
                pid = eid_to_pid.get(eid)
                if pid is None:
                    continue
                if val == "WON":
                    s.winner_pid = pid
                elif val == "LOST":
                    lost.add(pid)
                elif val == "TIED":
                    s.winner_pid = 0
            elif tag == "FIRST_PLAYER" and val:
                s.first_pid = eid_to_pid.get(eid, s.first_pid)
            elif tag == "CONTROLLER":
                ctrl_of[eid] = val

    if s.winner_pid is None and len(lost) == 1:
        # Concede often prints only the loser.
        other = [p for p in s.players if p not in lost]
        if len(other) == 1:
            s.winner_pid = other[0]

    # Class fallback: resolve the hero entity's card id through HSJSON.
    for pid, heid in s.hero_eid.items():
        if pid in s.classes:
            continue
        cid = card_of.get(heid)
        if cid:
            cls = _class_of_card(cid)
            if cls:
                s.classes[pid] = cls

    if game_meta:
        gt = game_meta.get("GameType")
        ft = game_meta.get("FormatType")
        s.build = game_meta.get("BuildNumber")
        base = MODE_BY_GAME_TYPE.get(gt, "unknown")
        if ft == FormatType.FT_STANDARD:
            s.fmt = "standard"
        elif ft == FormatType.FT_WILD:
            s.fmt = "wild"
        elif ft is not None and getattr(ft, "name", "") == "FT_TWIST":
            s.fmt = "twist"
        if base == "ranked" and s.fmt:
            s.mode = f"ranked_{s.fmt}"
        else:
            s.mode = base
    return s


_CARD_CLASS_CACHE = {}


def _class_of_card(card_id):
    """Hero card id -> class, from the bundled HSJSON corpus."""
    if not _CARD_CLASS_CACHE:
        here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        path = os.path.join(here, "hs2", "standard_cards.json")
        try:
            raw = json.load(open(path))
        except OSError:
            _CARD_CLASS_CACHE["__loaded__"] = True
            return None
        for cid, e in raw.items():
            _CARD_CLASS_CACHE[cid] = e.get("cls")
        _CARD_CLASS_CACHE["__loaded__"] = True
    cls = _CARD_CLASS_CACHE.get(card_id)
    if cls:
        return cls
    # Hero skins are named HERO_xx / HERO_xxy; strip the skin suffix.
    m = re.match(r"^(HERO_\d+)", card_id or "")
    if m:
        return _CARD_CLASS_CACHE.get(m.group(1))
    return None


def normalize_class(name):
    if not name:
        return None
    up = str(name).upper()
    try:
        return CardClass[up].name
    except KeyError:
        return up


# ------------------------------------------------------------------ import
def parsed_games(path, logs_dir=None):
    """Yield `(raw_slice_lines, events, summary, parser)` per game.

    The yielded slice is the **raw** one (both loggers): that is what gets
    gzipped into `games.raw_power`, so a future re-parse is not limited to
    the stream this build happened to prefer.
    """
    lines = read_lines(path)
    logger = choose_logger(lines)
    for raw in split_games(lines, boundary_logger(lines)):
        tree, parser = parse_slice(filter_logger(raw, logger), logger)
        if tree is None:
            continue
        events = events_from_tree(tree)
        summ = summarize(events, parser.game_meta)
        attach_names(summ, parser)
        yield raw, events, summ, parser


def attach_names(summ, parser):
    """Battletags are revealed *after* CREATE_GAME (the opponent shows up as
    `UNKNOWN HUMAN PLAYER`), so take the resolved names off hslog's player
    manager once the whole slice has been read."""
    mgr = getattr(parser, "player_manager", None)
    if mgr is None:
        return summ
    for pid in list(summ.players):
        try:
            ref = mgr.get_player_by_player_id(pid)
        except Exception:
            ref = None
        if ref is not None and ref.name:
            summ.players[pid]["name"] = ref.name
    return summ


def import_log(store, path, logs_dir=None, player_name=None,
               deckstring=None, only_last=False):
    """Import every game in one Power.log. Returns the new game ids.

    Idempotent per game slice: `games.log_hash` is the sha1 of the slice, so
    re-importing the same session inserts nothing.
    """
    path = os.path.abspath(path)
    logs_dir = logs_dir or os.path.dirname(path)
    zone_log = os.path.join(os.path.dirname(path), "Zone.log")
    us_pid = friendly_controller(zone_log)

    batches = list(parsed_games(path, logs_dir))
    if only_last:
        batches = batches[-1:]

    out = []
    for sl, events, summ, parser in batches:
        blob = "".join(sl)
        log_hash = hashlib.sha1(blob.encode("utf-8", "replace")).hexdigest()
        if store.game_id_for_hash(log_hash) is not None:
            continue
        pid = _pick_us(summ, us_pid, player_name)
        opp = 2 if pid == 1 else 1
        row = {
            "started_at": os.path.getmtime(path),
            "ended_at": time.time(),
            "mode": summ.mode,
            "format": summ.fmt,
            "player_name": (summ.players.get(pid) or {}).get("name"),
            "player_id": pid,
            "player_class": normalize_class(summ.classes.get(pid)),
            "opponent_name": (summ.players.get(opp) or {}).get("name"),
            "opponent_class": normalize_class(summ.classes.get(opp)),
            "deckstring": deckstring,
            "result": summ.result_for(pid),
            "turns": summ.turns,
            "going_first": 1 if summ.first_pid == pid else
                           (0 if summ.first_pid else None),
            "log_dir": logs_dir,
            "log_hash": log_hash,
            "created_at": time.time(),
        }
        gid = store.submit("create_game", row)
        store.submit("set_raw_power", gid,
                     gzip.compress(blob.encode("utf-8", "replace")))
        store.submit("add_events", gid, PARSE_GENERATION,
                     event_rows(events))
        # `pending` before any work, so a crash mid-review is resumable
        # from sqlite rather than from the in-memory JOBS dict.
        store.submit("upsert_review", gid, "pending")
        out.append(gid)
    return out


def event_rows(events):
    """Canonical events -> `events` table rows (seq, type, payload)."""
    return [{"seq": i, "type": ev["type"], "payload": ev,
             "ts_log": ev.get("ts")}
            for i, ev in enumerate(events)]


def _pick_us(summ, us_pid, player_name):
    """Zone.log wins; battletag is the fallback; controller 1 is the floor."""
    if us_pid in summ.players:
        return us_pid
    if player_name:
        for pid, info in summ.players.items():
            if info.get("name") == player_name:
                return pid
    return min(summ.players) if summ.players else 1


def newest_power_log(logs_dir):
    """Newest session directory with a Power.log that looks real."""
    best, best_mtime = None, -1
    if not logs_dir or not os.path.isdir(logs_dir):
        return None
    for name in os.listdir(logs_dir):
        cand = os.path.join(logs_dir, name, "Power.log")
        if not os.path.exists(cand):
            cand = os.path.join(logs_dir, name)
            if os.path.basename(cand) != "Power.log":
                continue
        try:
            st = os.stat(cand)
        except OSError:
            continue
        if st.st_size < MIN_POWER_BYTES:
            continue
        if st.st_mtime > best_mtime:
            best, best_mtime = cand, st.st_mtime
    return best
