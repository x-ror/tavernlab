"""Canonical events -> `VisibleState`: the running sum of the packets.

Why a hand-written reducer and not `hslog.export.EntityTreeExporter`:
the exporter hands back the *final* entity tree, so a minion that traded
on turn 4 only ever shows up in the graveyard.  A review needs the board
as it stood *before* each decision, and that only exists as a fold over
the packet stream (design §2.5, §2.6).

Two properties the rest of the pipeline leans on:

* **Independent of `hs2`.**  A game full of unimplemented cards must
  still reconstruct.  `hs2.carddata` is therefore imported lazily inside
  `implemented_gap()`, and its absence empties exactly that one list.
* **Tolerant.**  A missing tag, a tag the client invented after this was
  written, an entity referenced before it was created, a `BLOCK_END`
  with no start — every one of those degrades.  A malformed log costs a
  field, never the review.  Anything that still slips through lands in
  `Reconstructor.errors` instead of propagating.

`search_ok` is hard-wired False: MVP never reconstructs the trigger
graph, so nothing here may claim it did.  `lethal_ok` stays False too —
PR 8's `Hs2Mapper` owns that flag and sets it on the snapshot.
"""
import json
import os

from .types import EntityView, VisibleState

# Card ids only ever resolve to a name here; behaviour lives in `hs2`.
_CARDS_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "hs2", "standard_cards.json")

_CARDS = None

# Zones the reducer keeps entities in.  Anything else (an enchantment in
# PLAY, a card REMOVEDFROMGAME) still lives in the entity map, it just
# does not surface on a board or in a hand.
BOARD_TYPES = frozenset(("MINION", "LOCATION"))

# Tags projected onto dedicated `EntityView` fields.  Every *other* tag —
# TAUNT, DIVINE_SHIELD, FROZEN, and whatever numeric tag a new patch
# invented — is copied into `EntityView.tags` verbatim.
_FIELD_TAGS = frozenset((
    "ZONE", "ZONE_POSITION", "CONTROLLER", "ATK", "HEALTH", "DAMAGE",
    "COST",
))

# Pure client bookkeeping: never read by an evaluator, and between them
# they are most of the bytes in a fat entity.  Dropping them keeps a
# snapshot inside the design's 8-15 KB budget (§2.5, size budget).  This
# is a *known-noise* list, not a whitelist: an unrecognised tag is still
# kept verbatim.
_NOISE_TAGS = frozenset((
    "ENTITY_ID", "NUM_TURNS_IN_PLAY", "NUM_TURNS_IN_HAND", "PREDAMAGE",
    "LAST_AFFECTED_BY", "DISPLAYED_CREATOR", "COPIED_HINT", "PREMIUM",
    "SPAWN_TIME_COUNT", "CREATOR_DBID", "RARITY", "CUSTOMTEXT1",
    "CUSTOMTEXT2", "CUSTOMTEXT3", "COPIED_FROM_ENTITY_ID",
    "TAG_LAST_KNOWN_COST_IN_HAND", "FAKE_ZONE", "FAKE_ZONE_POSITION",
    "SPAWN_TIME_COUNT_2", "HIDDEN_CHOICE",
))

# Player-entity tags that become `VisibleState.mana[pid]`.
_MANA_TAGS = (("crystals", "RESOURCES"), ("used", "RESOURCES_USED"),
              ("temp", "TEMP_RESOURCES"), ("overload", "OVERLOAD_LOCKED"),
              ("overload_owed", "OVERLOAD_OWED"))


# ------------------------------------------------------------ card names
def _load_cards():
    """HSJSON corpus, loaded once.  Names only — `hs2` is not involved."""
    global _CARDS
    if _CARDS is None:
        try:
            with open(_CARDS_PATH, encoding="utf-8") as fh:
                _CARDS = json.load(fh)
        except (OSError, ValueError):
            # No corpus is survivable: every name is then None and the
            # reconstruction carries ids only.
            _CARDS = {}
    return _CARDS


def card_entry(card_id):
    if not card_id:
        return None
    return _load_cards().get(card_id)


def card_name(card_id):
    """`None` for a card id the corpus does not know (new set, or a
    token that never shipped in Standard)."""
    e = card_entry(card_id)
    return e.get("name") if e else None


def implemented_gap(card_ids):
    """Visible card ids `hs2` cannot simulate, as names where known.

    Imported lazily and defensively on purpose: `eval.visible` has to
    stay importable with no engine at all, and a missing engine must
    read as "no gap information", not as "every card is broken".
    """
    ids = [c for c in card_ids if c]
    if not ids:
        return []
    try:
        from hs2 import carddata
    except Exception:                       # noqa: BLE001 - optional dep
        return []
    defs = getattr(carddata, "DEFS", None)
    if not defs:
        try:
            carddata.build_defs()
        except Exception:                   # noqa: BLE001 - optional dep
            return []
        defs = getattr(carddata, "DEFS", None)
    if not defs:
        return []
    out = set()
    for cid in ids:
        d = defs.get(cid)
        if d is None or not getattr(d, "implemented", False):
            out.add(card_name(cid) or cid)
    return sorted(out)


# --------------------------------------------------------------- helpers
def _int(v, default=0):
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, int):
        return v
    try:
        return int(str(v).strip())
    except (TypeError, ValueError):
        return default


def _zone(v):
    """hslog hands zones back as names; a raw int is still accepted."""
    if v is None:
        return None
    if isinstance(v, str):
        return v
    try:
        from hearthstone.enums import Zone
        return Zone(int(v)).name
    except Exception:                       # noqa: BLE001 - tolerant
        return str(v)


class _Entity:
    """One log entity: its (possibly hidden) card id plus every tag.

    `slot_seq` is the seq at which this entity last claimed a zone or a
    board slot.  It only exists to referee the board-slot repair in
    `Reconstructor._repair_board`.
    """

    __slots__ = ("eid", "card_id", "tags", "slot_seq")

    def __init__(self, eid):
        self.eid = eid
        self.card_id = None
        self.tags = {}
        self.slot_seq = -1

    def zone(self):
        return _zone(self.tags.get("ZONE")) or "SETASIDE"

    def cardtype(self):
        v = self.tags.get("CARDTYPE")
        return v if isinstance(v, str) else None

    def controller(self):
        return _int(self.tags.get("CONTROLLER"), 0)

    def view(self):
        t = self.tags
        return EntityView(
            eid=self.eid,
            card_id=self.card_id,
            name=card_name(self.card_id),
            controller=self.controller(),
            zone=self.zone(),
            zone_pos=_int(t.get("ZONE_POSITION"), 0),
            atk=t.get("ATK") if isinstance(t.get("ATK"), int) else None,
            health=(t.get("HEALTH")
                    if isinstance(t.get("HEALTH"), int) else None),
            damage=_int(t.get("DAMAGE"), 0),
            cost=t.get("COST") if isinstance(t.get("COST"), int) else None,
            tags={k: v for k, v in t.items()
                  if k not in _FIELD_TAGS and k not in _NOISE_TAGS},
        )


MAX_BOARD = 7


def _repair_board(ents):
    """Enforce Hearthstone's own board invariant on one side.

    A side has at most seven cards in play, each holding a distinct
    `ZONE_POSITION`.  This is a guard, not a repair shop: on all three
    fixtures it is a pure pass-through, so a firing is a *signal* — the
    fold lost a card's exit from play and the reconstruction is already
    wrong upstream of here.  `test_the_board_repair_never_has_to_fire`
    pins that, and `test_board_invariants_hold_without_the_repair`
    checks the fold obeys the rule with this bypassed entirely.

    It still resolves rather than raises, so a downstream consumer is
    handed a legal board instead of an impossible one: the freshest
    claim on a slot wins, and anything over the cap goes stalest-first.

    Only settled positions are asked for.  Mid-fold two entities do
    briefly share a slot — the client renumbers a board one `TAG_CHANGE`
    at a time — but `state()` is called at decision points and turn
    boundaries, where the renumbering has finished.
    """
    by_slot, loose = {}, []
    for e in sorted(ents, key=lambda x: x.eid):
        pos = _int(e.tags.get("ZONE_POSITION"), 0)
        if pos <= 0:
            loose.append(e)             # mid-play, no slot assigned yet
            continue
        cur = by_slot.get(pos)
        if cur is None or (e.slot_seq, e.eid) > (cur.slot_seq, cur.eid):
            by_slot[pos] = e
    out = [by_slot[p] for p in sorted(by_slot)] + loose
    if len(out) > MAX_BOARD:
        keep = sorted(out, key=lambda e: e.slot_seq)[-MAX_BOARD:]
        out = [e for e in out if e in keep]
    return out


class Reconstructor:
    """Fold canonical events into an entity map; emit `VisibleState`.

    `apply()` takes one event, `run()` takes them all.  `state(seq)` and
    `snapshot()` build a fresh `VisibleState` every call — callers are
    expected to mutate what they get back, so nothing internal is ever
    handed out by reference.
    """

    def __init__(self, us=1):
        self.us = _int(us, 1) or 1
        self.reset()

    # -- lifecycle ------------------------------------------------------
    def reset(self):
        self.entities = {}          # eid -> _Entity
        self.player_eid = {}        # player_id -> player entity id
        self.pid_of = {}            # player entity id -> player_id
        self.game_eid = None
        self.turn = 0
        self.current_player = 0
        self.first_player = None
        self.playstate = {}         # player_id -> PLAYING|WON|LOST|TIED
        self.seq = -1
        self.depth = 0              # BLOCK nesting; 0 == top level
        self.choices = {}           # choice id -> the CHOICES event
        self.errors = []            # (seq, type, repr(exc)) - never raised

    def ent(self, eid):
        """Get-or-create.  A TAG_CHANGE for an entity we never saw
        created is normal (the log skips entities the client already
        knows), so it makes one rather than dropping the tag."""
        if eid is None:
            return None
        e = self.entities.get(eid)
        if e is None:
            e = self.entities[eid] = _Entity(eid)
        return e

    # -- reduction ------------------------------------------------------
    def apply(self, ev, seq=None):
        """One canonical event.  Never raises: a bad event is recorded
        in `self.errors` and the fold continues."""
        self.seq = (self.seq + 1) if seq is None else seq
        if not isinstance(ev, dict):
            self.errors.append((self.seq, None, "not a dict"))
            return
        kind = ev.get("type")
        try:
            fn = _HANDLERS.get(kind)
            if fn is not None:
                fn(self, ev)
        except Exception as exc:            # noqa: BLE001 - tolerated
            self.errors.append((self.seq, kind, repr(exc)))

    def run(self, events):
        for i, ev in enumerate(events):
            self.apply(ev, i)
        return self

    # -- event handlers -------------------------------------------------
    def _on_create_game(self, ev):
        """A new game wipes the map and registers both players."""
        seq = self.seq
        self.reset()
        self.seq = seq              # the fold's position is not game state
        self.game_eid = ev.get("entity_id")
        g = self.ent(self.game_eid)
        if g is not None:
            g.tags.update(ev.get("tags") or {})
            self.turn = _int(g.tags.get("TURN"), 0)
        for pl in ev.get("players") or []:
            eid, pid = pl.get("entity_id"), pl.get("player_id")
            if eid is None or pid is None:
                continue
            self.player_eid[pid] = eid
            self.pid_of[eid] = pid
            e = self.ent(eid)
            e.tags.update(pl.get("tags") or {})
            e.tags.setdefault("CONTROLLER", pid)
            self._player_side_effects(pid, e.tags)

    def _player_side_effects(self, pid, tags):
        if tags.get("FIRST_PLAYER"):
            self.first_player = pid
        if tags.get("CURRENT_PLAYER"):
            self.current_player = pid
        if tags.get("PLAYSTATE"):
            self.playstate[pid] = tags["PLAYSTATE"]

    def _on_reset_game(self, ev):
        """`RESET_GAME` restarts the entity map without a CREATE_GAME."""
        players, seq = dict(self.player_eid), self.seq
        self.reset()
        self.seq = seq
        for pid, eid in players.items():
            self.player_eid[pid] = eid
            self.pid_of[eid] = pid
            self.ent(eid).tags["CONTROLLER"] = pid

    def _on_full_entity(self, ev):
        e = self.ent(ev.get("entity_id"))
        if e is None:
            return
        if ev.get("card_id"):
            e.card_id = ev["card_id"]
        for tag, val in (ev.get("tags") or {}).items():
            self._set(e, tag, val)

    def _on_hide_entity(self, ev):
        """The card went face down again: it keeps its identity in the
        entity map but stops revealing a card id."""
        e = self.ent(ev.get("entity_id"))
        if e is None:
            return
        e.card_id = None
        if ev.get("zone") is not None:
            self._set(e, "ZONE", ev["zone"])

    def _on_tag_change(self, ev):
        e = self.ent(ev.get("entity_id"))
        if e is None:
            return
        self._set(e, ev.get("tag"), ev.get("value"))

    def _set(self, e, tag, value):
        """Write one tag and mirror the game-wide ones."""
        if tag is None:
            return
        e.tags[tag] = value
        if tag in ("ZONE", "ZONE_POSITION"):
            e.slot_seq = self.seq
        if tag == "TURN":
            # The *GameEntity* TURN is `VisibleState.turn`; a player's own
            # TURN tag counts that player's turns and must not be mixed in.
            if e.eid == self.game_eid:
                self.turn = _int(value, self.turn)
            return
        pid = self.pid_of.get(e.eid)
        if pid is None:
            return
        if tag == "CURRENT_PLAYER":
            if _int(value, 0):
                self.current_player = pid
        elif tag == "FIRST_PLAYER":
            if _int(value, 0):
                self.first_player = pid
        elif tag == "PLAYSTATE":
            self.playstate[pid] = value

    def _on_block_start(self, ev):
        self.depth += 1

    def _on_block_end(self, ev):
        # An orphaned BLOCK_END happens in real logs (hslog logs
        # "Orphaned BLOCK_END" and recovers); clamp rather than go
        # negative, which would make every later block look top level.
        self.depth = max(0, self.depth - 1)

    def _on_choices(self, ev):
        cid = ev.get("id")
        if cid is not None:
            self.choices[cid] = ev

    # `META_DATA`, `SEND_CHOICES`, `CHOSEN_ENTITIES`, `OPTIONS`,
    # `SEND_OPTION`, `SUB_SPELL`, `SHUFFLE_DECK` and `OTHER` carry no
    # state: the tags they imply are always logged separately.  They are
    # deliberately absent from `_HANDLERS` rather than no-op methods.

    # -- projection -----------------------------------------------------
    def hero_eid(self, pid):
        """The player's HERO_ENTITY tag, else the newest HERO in play.

        A Death Knight / Galakrond hero card replaces the entity, and the
        tag is the only reliable pointer once that has happened.
        """
        peid = self.player_eid.get(pid)
        if peid is not None:
            tag = self.entities[peid].tags.get("HERO_ENTITY") \
                if peid in self.entities else None
            if _int(tag, 0) in self.entities:
                return _int(tag, 0)
        best = None
        for eid, e in self.entities.items():
            if e.cardtype() == "HERO" and e.controller() == pid \
                    and e.zone() == "PLAY":
                best = eid if best is None else max(best, eid)
        return best

    def _hero(self, pid):
        eid = self.hero_eid(pid)
        e = self.entities.get(eid)
        if e is None:
            return {}
        t = e.tags
        max_hp, dmg = _int(t.get("HEALTH"), 0), _int(t.get("DAMAGE"), 0)
        return {
            "eid": eid,
            "card_id": e.card_id,
            "name": card_name(e.card_id),
            # Remaining health is a bounded quantity — a hero is dead
            # at 0 — so it is clamped.  The raw pair only goes negative
            # after the killing blow, when the client resets a dead
            # hero's HEALTH to its base while DAMAGE still carries the
            # overkill (real_game2 ends 40 damage against 30 health, on
            # a hero already in the GRAVEYARD and a PLAYSTATE already
            # LOSING).  No live position reaches here negative, so this
            # shapes the epilogue rather than papering over lost
            # packets.  `max_hp` and `damage` keep the raw pair, and
            # `test_hero_hp_never_needs_the_clamp_at_a_decision_point`
            # pins that the clamp is inert wherever a review looks.
            "hp": max(0, max_hp - dmg),
            "max_hp": max_hp,
            "damage": dmg,
            "armor": _int(t.get("ARMOR"), 0),
            "atk": _int(t.get("ATK"), 0),
            "frozen": bool(_int(t.get("FROZEN"), 0)),
            "immune": bool(_int(t.get("IMMUNE"), 0)),
            "attacks": _int(t.get("NUM_ATTACKS_THIS_TURN"), 0),
            "cls": t.get("CLASS"),
        }

    def _hero_power(self, pid):
        """The player's *current* hero power.

        A replaced power (Justicar, a hero card) is moved to SETASIDE and
        the new one takes its place in PLAY, so "CARDTYPE=HERO_POWER,
        controller=pid, zone=PLAY" identifies exactly one entity. Its
        EXHAUSTED tag is the only record that the power was already spent
        this turn — without it a lethal search counts a Fireblast that
        the player had no way left to fire.
        """
        for eid in sorted(self.entities):
            e = self.entities[eid]
            if e.cardtype() != "HERO_POWER" or e.controller() != pid:
                continue
            if e.zone() != "PLAY":
                continue
            t = e.tags
            return {"eid": eid, "card_id": e.card_id,
                    "name": card_name(e.card_id),
                    "cost": _int(t.get("COST"), 2),
                    "exhausted": bool(_int(t.get("EXHAUSTED"), 0)),
                    "passive": bool(_int(t.get("HIDE_STATS"), 0))}
        return None

    def _weapon(self, pid):
        peid = self.player_eid.get(pid)
        eid = None
        if peid in self.entities:
            eid = _int(self.entities[peid].tags.get("WEAPON"), 0) or None
        if eid not in self.entities:
            eid = None
            for cand, e in self.entities.items():
                if e.cardtype() == "WEAPON" and e.controller() == pid \
                        and e.zone() == "PLAY":
                    eid = cand
        if eid is None:
            return None
        e = self.entities[eid]
        t = e.tags
        dur = t.get("DURABILITY")
        if dur is None:
            # Modern logs carry a weapon's durability in HEALTH.
            dur = t.get("HEALTH")
        left = _int(dur, 0) - _int(t.get("DAMAGE"), 0)
        return {"eid": eid, "card_id": e.card_id,
                "name": card_name(e.card_id),
                "atk": _int(t.get("ATK"), 0),
                # `dur` is what `eval.mapper` reads; `durability` is the
                # spelled-out alias for the UI.
                "dur": left, "durability": left,
                "windfury": bool(_int(t.get("WINDFURY"), 0)),
                "lifesteal": bool(_int(t.get("LIFESTEAL"), 0))}

    def _mana(self, pid):
        peid = self.player_eid.get(pid)
        t = self.entities[peid].tags if peid in self.entities else {}
        return {key: _int(t.get(tag), 0) for key, tag in _MANA_TAGS}

    def _quest(self, pid):
        """The player's quest / sidequest, if one is visible."""
        for eid in sorted(self.entities):
            e = self.entities[eid]
            if e.controller() != pid:
                continue
            t = e.tags
            if not (_int(t.get("QUEST"), 0) or _int(t.get("SIDEQUEST"), 0)):
                continue
            if e.zone() not in ("PLAY", "SECRET"):
                continue
            return {"eid": eid, "card_id": e.card_id,
                    "name": card_name(e.card_id),
                    "sidequest": bool(_int(t.get("SIDEQUEST"), 0)),
                    "progress": _int(t.get("QUEST_PROGRESS"), 0),
                    "total": _int(t.get("QUEST_PROGRESS_TOTAL"), 0)}
        return None

    def state(self, seq=None):
        """A fresh `VisibleState` — every container is newly built, so
        the caller owns it outright and may mutate it."""
        pids = sorted(self.player_eid) or [1, 2]
        vs = VisibleState()
        vs.seq = self.seq if seq is None else seq
        vs.turn = self.turn
        vs.current_player = self.current_player or (self.first_player or 1)
        vs.us = self.us

        for pid in pids:
            vs.mana[pid] = self._mana(pid)
            vs.heroes[pid] = self._hero(pid)
            w = self._weapon(pid)
            if w:
                vs.weapons[pid] = w
            hp = self._hero_power(pid)
            if hp:
                vs.hero_powers[pid] = hp
            vs.boards[pid] = []
            vs.hands[pid] = []
            vs.secrets[pid] = []
            vs.deck_counts[pid] = 0
            peid = self.player_eid.get(pid)
            corpses = self.entities[peid].tags.get("CORPSES") \
                if peid in self.entities else None
            vs.corpses[pid] = _int(corpses, 0)
            q = self._quest(pid)
            if q:
                vs.quest[pid] = q

        heroes = {vs.heroes[p].get("eid") for p in pids}
        on_board = {pid: [] for pid in pids}
        for eid in sorted(self.entities):
            e = self.entities[eid]
            pid = e.controller()
            if pid not in vs.boards:
                continue
            if eid in self.pid_of or eid == self.game_eid or eid in heroes:
                continue
            zone, ctype = e.zone(), e.cardtype()
            if zone == "PLAY" and ctype in BOARD_TYPES:
                on_board[pid].append(e)
            elif zone == "HAND":
                vs.hands[pid].append(e.view())
            elif zone == "SECRET":
                vs.secrets[pid].append({"eid": eid, "card_id": e.card_id,
                                        "name": card_name(e.card_id)})
            elif zone == "DECK":
                vs.deck_counts[pid] += 1
        for pid in pids:
            vs.boards[pid] = [e.view() for e in _repair_board(on_board[pid])]
            vs.hands[pid].sort(key=lambda v: (v.zone_pos, v.eid))

        # Only what *we* can see feeds the gap: our own hand plus both
        # boards.  The opponent's hand is face down by definition.
        seen = [v.card_id for v in vs.hands.get(vs.us, [])]
        for pid in pids:
            seen += [v.card_id for v in vs.boards[pid]]
        vs.implemented_gap = implemented_gap(seen)

        # MVP invariant (design §2.6): the trigger graph is never
        # reconstructed, so `search_ok` is 0 and no skill label may ship.
        # `lethal_ok` is PR 8's to raise, on the snapshot, not here.
        vs.search_ok = False
        vs.lethal_ok = False
        return vs

    def snapshot(self):
        return self.state()


_HANDLERS = {
    "CREATE_GAME": Reconstructor._on_create_game,
    "RESET_GAME": Reconstructor._on_reset_game,
    "FULL_ENTITY": Reconstructor._on_full_entity,
    "SHOW_ENTITY": Reconstructor._on_full_entity,
    "CHANGE_ENTITY": Reconstructor._on_full_entity,
    "HIDE_ENTITY": Reconstructor._on_hide_entity,
    "TAG_CHANGE": Reconstructor._on_tag_change,
    "BLOCK_START": Reconstructor._on_block_start,
    "BLOCK_END": Reconstructor._on_block_end,
    "CHOICES": Reconstructor._on_choices,
}


def reconstruct(events, us_pid=1):
    """Convenience: fold every event, hand back the final state."""
    return Reconstructor(us_pid).run(events).state()
