"""PR 10: turn ledger + hatched WP series + key moments -> the Review JSON.

The contract this produces is design §2.7's "Review JSON (contract for
UI)".  Three rules it exists to enforce:

* every `logistic_v1` point is **hatched**, including mapped plies — the
  9 weights ignore card identity, quests, combo and hidden cards;
* `delta_wp` may be stored but never ranks or labels a play;
* key moments are missed lethal plus ledger notes, **not** top |ΔWP|.
"""
import time

from eval import classify as cl
from eval import ledger as ledger_mod
from eval.mapper import fields_parse
from eval import taggers
from eval.solvers import lethal as lethal_solver
from eval.types import Explanation

WP_SOURCE = "logistic_v1"
MAX_LEDGER_MOMENTS = 3

# Design §4.6: a review job is bounded. The budget is cooperative rather
# than a killed thread — the ledger and the WP series are cheap and always
# finish; only the deep lethal probe can run long, so that is what gets
# switched off, and the report says so instead of silently thinning out.
REVIEW_TIMEOUT_S = 60.0

# Deep lethal costs clones, so probe where a missed lethal actually hides:
# the first *real* action of our turn (full mana) and the last one (what
# was left). A `turn_start` snapshot is taken **before** the TURN tag is
# applied, so its mana still shows last turn's spend — probing it would
# look for lethal with zero mana and find nothing.
LETHAL_PROBE_KINDS = ("play", "attack", "hero_power", "location")


def build_review(events, game=None, us_pid=None, deep=True,
                 evaluator_version=cl.EVALUATOR_VERSION,
                 timeout_s=REVIEW_TIMEOUT_S):
    """Assemble the whole review from canonical events.

    `game` is the `games` row (or any mapping) — it supplies the classes
    and the result. Everything else is derived.

    Returns `(review, decision_rows, snapshot_rows)`. The snapshot rows
    come back stamped with the win probability and `lethal_ok`, which is
    the only place both are known: the reconstructor has no evaluator and
    the evaluator does not own the store.
    """
    from eval.snapshots import build_snapshots, snapshot_rows

    game = dict(game or {})
    us_pid = us_pid or game.get("player_id") or 1
    us_cls = game.get("player_class")
    them_cls = game.get("opponent_class")

    states, points = build_snapshots(events, us_pid)
    outcome = read_outcome(events, us_pid)

    wp_series, wps = [], []
    for vs, pt in zip(states, points):
        wps.append(_wp(vs, us_pid))
        wp_series.append({
            "seq": pt.get("seq"), "turn": _turn_of(vs, pt),
            "wp": round(wps[-1], 4),
            "source": WP_SOURCE,
            # Non-negotiable: this model is not a play-ranking oracle.
            "hatch": True,
        })

    deadline = (time.monotonic() + timeout_s) if timeout_s else None
    ctx = {"going_first": _going_first(game),
           "archetype": game.get("opponent_archetype")}
    turns, decisions, moments, lethal_ok_by_seq, timed_out = _walk_turns(
        states, points, us_pid, us_cls, them_cls, outcome, deep,
        evaluator_version, deadline, ctx)

    snaps = snapshot_rows(states, points)
    for row, wp, vs in zip(snaps, wps, states):
        row["wp"] = round(wp, 4)
        row["wp_source"] = WP_SOURCE
        row["lethal_ok"] = 1 if fields_parse(vs, us_pid) else 0

    result = game.get("result") or outcome["result"]
    report = _report(turns, moments, result, outcome, timed_out)
    return {
        "game_id": game.get("id"),
        "status": "partial" if timed_out else "ready",
        "result": result,
        "matchup": {"us": us_cls, "them": them_cls,
                    "archetype": game.get("opponent_archetype"),
                    "conf": game.get("opponent_archetype_conf")},
        "wp_series": wp_series,
        "key_moments": moments,
        "turns": turns,
        "report": report,
        "labels_legend": cl.legend(),
        "evaluator_version": evaluator_version,
        "generated_at": time.time(),
    }, decisions, snaps


def _wp(vs, us_pid):
    from hs2.winprob import wp_from_visible
    try:
        return wp_from_visible(vs, us_pid)
    except Exception:
        return 0.5


def _int(v, default=0):
    return v if isinstance(v, int) and not isinstance(v, bool) else default


def read_outcome(events, us_pid):
    """Who won, on which turn, and did *we* land the kill.

    `lethal_turn` is what turns "missed lethal" into "played lethal": the
    turn our opponent's PLAYSTATE went LOST.

    Only the **GameEntity** carries the game-wide turn counter. Each
    player also has a TURN tag counting *its own* turns, and the client
    emits it right before PLAYSTATE: on a real 13-turn game the tail
    reads `GameEntity TURN=13`, `Player TURN=7`, `WON`. Counting both
    left `lethal_turn=7` while the killing decision sat in bucket 13, so
    the turn you actually won on was reported as a missed lethal —
    exactly the false positive the design calls product-ending.
    `eval/visible.py` has always filtered this; `read_outcome` did not.
    """
    out = {"winner_pid": None, "result": "unknown", "end_turn": None,
           "lethal_turn": None}
    eid_to_pid, turn, game_eid = {}, 0, 1
    for ev in events:
        t = ev.get("type")
        if t == "CREATE_GAME":
            game_eid = ev.get("entity_id") or 1
            turn = _int(((ev.get("tags") or {}).get("TURN")), turn)
            for pl in ev.get("players", []):
                eid_to_pid[pl.get("entity_id")] = pl.get("player_id")
        elif t == "TAG_CHANGE":
            if ev.get("tag") == "TURN" and ev.get("entity_id") == game_eid \
                    and isinstance(ev.get("value"), int):
                turn = ev["value"]
            elif ev.get("tag") == "PLAYSTATE":
                pid = eid_to_pid.get(ev.get("entity_id"))
                if pid is None:
                    continue
                if ev.get("value") == "WON":
                    out["winner_pid"] = pid
                    out["end_turn"] = turn
                    if pid == us_pid:
                        out["lethal_turn"] = turn
                elif ev.get("value") == "TIED":
                    out["winner_pid"] = 0
                    out["end_turn"] = turn
    if out["winner_pid"] == 0:
        out["result"] = "tie"
    elif out["winner_pid"] is not None:
        out["result"] = "win" if out["winner_pid"] == us_pid else "loss"
    return out


def _going_first(game):
    v = game.get("going_first")
    return None if v is None else bool(v)


def _walk_turns(states, points, us_pid, us_cls, them_cls, outcome, deep,
                version, deadline=None, ctx=None):
    rows = _normalise_sides(list(zip(states, points)), us_pid)
    # Group by the *decision point's* turn, not the snapshot's. A
    # turn-start snapshot is the state before the TURN tag lands, so
    # `vs.turn` still holds the previous number and grouping on it would
    # split every turn in two — and hang a label on a turn that never
    # happened.
    by_turn = {}
    for vs, pt in rows:
        by_turn.setdefault(_turn_of(vs, pt), []).append((vs, pt))

    turns, decisions, moments, lethal_ok_by_seq = [], [], [], {}
    timed_out = False
    order = sorted(by_turn)
    # Belt and braces on top of the GameEntity-only turn counter: if we
    # won, the last turn we acted on *is* the turn we killed on, whatever
    # order the client happened to print the closing tags in. Without a
    # second source here, one odd log resurrects "missed lethal on the
    # turn you won".
    our_turns = [t for t in order
                 if any(pt.get("side") == "us" for _vs, pt in by_turn[t])]
    kill_turn = (our_turns[-1] if outcome.get("result") == "win"
                 and our_turns else None)
    for i, turn in enumerate(order):
        rows = by_turn[turn]
        ours = [(vs, pt) for vs, pt in rows if pt.get("side") == "us"]
        end_vs = _end_state(by_turn, order, i)
        led = ledger_mod.build(
            end_vs, us_pid=us_pid,
            side="us" if ours else "them",
            hero_power_used=_hero_power_used(rows))
        if deadline is not None and time.monotonic() > deadline:
            # Keep walking: the ledger and WP series still cost nothing.
            timed_out, deep = True, False
        finding, found_seq = None, None
        if ours:
            finding, found_seq = _probe_lethal(
                ours, us_pid, us_cls, them_cls, outcome, turn, deep,
                taken=(outcome.get("lethal_turn") == turn
                       or kill_turn == turn))
            led.lethal = bool(finding and finding.available)

        turn_decisions = []
        for vs, pt in rows:
            # The label belongs to the one decision where the lethal was
            # on the board, not to every ply of the turn.
            f = finding if pt.get("seq") == found_seq else None
            row = _decision(vs, pt, us_pid, f, version, ctx)
            turn_decisions.append(row["public"])
            decisions.append(row["store"])

        turns.append({"turn": turn, "ledger": led.to_dict(),
                      "decisions": turn_decisions})

        if finding is not None and finding.missed:
            moments.append({
                "seq": found_seq, "turn": turn,
                "title": "Missed lethal", "label": "missed_lethal",
                "detail": finding.plan,
                "approx": finding.approx,
                "conf": 0.75 if finding.approx else 0.95})

    if outcome["result"] == "loss":
        moments.extend(_ledger_moments(turns))
    return turns, decisions, moments, lethal_ok_by_seq, timed_out


def _end_state(by_turn, order, i):
    """The state at the *end* of `order[i]`.

    A turn-start snapshot is taken before the TURN tag lands, so the
    first row of the next turn's group is exactly this turn's final
    board — mana spent, attacks used. Falling back to this turn's last
    row would account for the position *before* its last play.
    """
    if i + 1 < len(order):
        nxt = by_turn[order[i + 1]][0]
        if nxt[1].get("kind") == "turn_start":
            return nxt[0]
    return by_turn[order[i]][-1][0]


def _turn_of(vs, pt):
    t = pt.get("turn")
    return int(t) if isinstance(t, int) else int(vs.turn or 0)


def _normalise_sides(rows, us_pid):
    """Fix the side on `turn_start` points.

    A turn-start snapshot is the state **before** the TURN tag is
    applied, so its `current_player` — and therefore the reducer's `side`
    — still names the player who just finished. Take the side from the
    next real decision of the same turn instead; Hearthstone turns
    strictly alternate, so the last one falls back to flipping.
    """
    out = list(rows)
    for i, (vs, pt) in enumerate(out):
        if pt.get("kind") != "turn_start":
            continue
        pt = dict(pt)
        nxt = next((q for _v, q in out[i + 1:]
                    if q.get("kind") != "turn_start"), None)
        if nxt is not None:
            pt["side"] = nxt.get("side")
        else:
            pt["side"] = "them" if pt.get("side") == "us" else "us"
        out[i] = (vs, pt)
    return out


def _hero_power_used(rows):
    for _vs, pt in rows:
        if pt.get("kind") == "hero_power" and pt.get("side") == "us":
            return True
    return None if not rows else False


def _probe_lethal(ours, us_pid, us_cls, them_cls, outcome, turn, deep,
                  taken=False):
    """Best finding across the probe positions on our turn."""
    acted = [(vs, pt) for vs, pt in ours
             if pt.get("kind") in LETHAL_PROBE_KINDS]
    picks = acted or ours
    positions = [picks[0]] if len(picks) == 1 else [picks[0], picks[-1]]
    seen = []
    for vs, pt in positions:
        f = lethal_solver.detect(vs, us_cls=us_cls, them_cls=them_cls,
                                 us_pid=us_pid, taken=taken, deep=deep)
        seen.append((f, pt.get("seq")))
        if f.available:
            return f, pt.get("seq")
    return seen[0]


def _decision(vs, pt, us_pid, finding, version, ctx=None):
    """One decision row: the public shape and the `decisions` row."""
    side = pt.get("side") or ("us" if vs.current_player == us_pid
                              else "them")
    lethal_d = finding.to_dict() if (finding and side == "us") else {}
    # `lethal_ok` is a property of the state, not of whether this ply
    # happened to be one of the two we probed — otherwise an unprobed
    # ply shows the UI's "lethal off" badge for no reason.
    lethal_ok = fields_parse(vs, us_pid)
    gates = cl.gates_for(lethal=dict(lethal_d, lethal_ok=lethal_ok)
                         if lethal_d else {},
                         actions_complete=False, search_ok=False)
    verdict = cl.classify(gates) if side == "us" else cl.Verdict(
        None, None, "opponent decision")

    chosen = {"card": pt.get("name") or pt.get("card_id"),
              "card_id": pt.get("card_id"),
              "kind": pt.get("kind"), "entity_id": pt.get("entity_id"),
              "target_id": pt.get("target_id")}
    if pt.get("kind") == "mulligan":
        chosen["choices"] = _mulligan_choices(pt)
    expl = _explain(pt, finding if side == "us" else None, verdict,
                    vs if side == "us" else None, us_pid, ctx, chosen)

    public = {
        "seq": pt.get("seq"), "kind": pt.get("kind"), "side": side,
        "chosen": chosen,
        # Stored for later calibration, never used to rank or label.
        "delta_wp": None,
        "label": verdict.label,
        "label_conf": verdict.conf,
        "label_reason": verdict.reason,
        "actions_complete": False,
        "lethal_ok": lethal_ok,
        # Whether the lethal solver actually ran here. Without it,
        # `lethal_ok=1, lethal_available=0` reads as "checked, none
        # found" on a ply nobody checked.
        "lethal_checked": bool(lethal_d),
        "search_ok": False,
        "explanation": expl.to_dict(),
    }
    store = {
        "event_seq": pt.get("seq"), "turn": _turn_of(vs, pt),
        "side": side,
        "kind": pt.get("kind") or "play",
        "chosen": chosen, "alternatives": None,
        "actions_complete": 0,
        "lethal_ok": 1 if lethal_ok else 0,
        "search_ok": 0,
        "wp_before": round(_wp(vs, us_pid), 4),
        "wp_after": None, "delta_wp": None,
        "label": verdict.label, "label_conf": verdict.conf,
        "lethal_available": 1 if lethal_d.get("available") else 0,
        "lethal_plan": lethal_d.get("plan"),
        "explanation": expl.to_dict(),
        "search_depth": 0,
        "evaluator_version": version,
    }
    return {"public": public, "store": store}


def _mulligan_choices(pt):
    """The offered set with names and costs, so the mulligan tagger and
    the UI both see what was passed as well as what was kept."""
    from eval.visible import card_entry
    out = []
    for c in pt.get("choices") or []:
        entry = card_entry(c.get("card_id")) or {}
        out.append({"eid": c.get("eid"), "card_id": c.get("card_id"),
                    "picked": bool(c.get("picked")),
                    "name": entry.get("name"),
                    "cost": entry.get("cost")})
    return out


def _explain(pt, finding, verdict, vs=None, us_pid=None, ctx=None,
             chosen=None):
    name = pt.get("name") or pt.get("card_id") or pt.get("kind") or "action"
    what = {"play": f"Played {name}", "attack": f"Attacked with {name}",
            "hero_power": "Used hero power",
            "location": f"Used {name}",
            "mulligan": "Mulligan", "discover": f"Discover from {name}",
            "turn_start": "Turn start"}.get(pt.get("kind"),
                                            f"{pt.get('kind')} {name}")
    e = Explanation(what=what, tags=[pt.get("kind") or "action"])
    if finding is not None and finding.missed:
        e.why_bad.append(f"Lethal was available: {finding.plan}")
        e.tags.append("lethal")
        if finding.approx:
            e.caveats.append(
                "The lethal line was found by a bounded search and is "
                "not proven exact.")
    if verdict.label is None and verdict.reason:
        e.caveats.append(verdict.reason)
    if finding is not None and not finding.hand_complete:
        e.caveats.append(
            "Some cards in hand are not implemented, so 'no lethal' "
            "here means unknown, not proven absent.")
    _add_strategic(e, pt, vs, us_pid, ctx, chosen)
    return e


def _add_strategic(e, pt, vs, us_pid, ctx, chosen):
    """Only taggers with checkable evidence; a wrong strategic tag is
    worse than none (design §3.6)."""
    ctx = ctx or {}
    if pt.get("kind") == "mulligan":
        tag = taggers.mulligan_keep(
            (chosen or {}).get("choices"),
            going_first=ctx.get("going_first"),
            archetype=ctx.get("archetype"))
        if tag:
            e.strategic.append(tag["text"])
            e.tags.append(tag["tag"])
        return
    if vs is None:
        return
    for tag in taggers.all_tags(vs, us_pid):
        e.strategic.append(tag["text"])
        e.tags.append(tag["tag"])


def _ledger_moments(turns):
    """Up to 3 ledger notes on a loss. **Not** top |ΔWP| (design §3.5)."""
    scored = []
    for t in turns:
        led = t["ledger"]
        if led.get("side") != "us":
            continue
        weight = led.get("mana_left", 0) + 2 * led.get("unused_attacks", 0)
        if weight <= 0:
            continue
        # Anchor the note on the turn's last ply so "click a key moment"
        # can scroll to a real seq, the same as a missed lethal.
        seqs = [d.get("seq") for d in t.get("decisions") or []
                if d.get("seq") is not None]
        anchor = seqs[-1] if seqs else None
        for note in led.get("notes", []):
            scored.append((weight, {
                "seq": anchor, "turn": t["turn"],
                "title": note.get("what"), "label": "note",
                "detail": "; ".join(note.get("why_bad") or []),
                "conf": None}))
    scored.sort(key=lambda x: -x[0])
    return [m for _w, m in scored[:MAX_LEDGER_MOMENTS]]


def _report(turns, moments, result, outcome, timed_out=False):
    missed = [m for m in moments if m["label"] == "missed_lethal"]
    bullets, caveats = [], [cl.wp_caveat()]
    if missed:
        first = missed[0]
        headline = (f"Thrown on turn {first['turn']}: lethal was on the "
                    f"board.")
        for m in missed:
            bullets.append(f"Missed lethal on turn {m['turn']} "
                           f"({m['detail']}).")
        if any(m.get("approx") for m in missed):
            caveats.append(
                "A lethal marked approximate came from a bounded taunt "
                "search; the line is plausible, not proven.")
    elif result == "loss":
        headline = "No missed lethal found; the leaks were smaller."
    elif result == "win":
        headline = "Won. Nothing critical flagged."
    else:
        headline = "Game reviewed."

    for m in moments:
        if m["label"] == "note" and m["detail"]:
            bullets.append(f"Turn {m['turn']}: {m['detail']}")
    caveats.append(
        "search_ok=0 in this build: no play is ranked, and no "
        "Mistake/Blunder/Best label is produced.")
    if timed_out:
        caveats.append(
            f"The lethal search hit its {int(REVIEW_TIMEOUT_S)} s budget "
            f"and was switched off partway through; later turns were not "
            f"checked for a missed lethal.")
    if outcome.get("result") == "unknown":
        caveats.append("The log ends before a winner was recorded.")
    return {"headline": headline, "bullets": bullets[:6],
            "caveats": caveats}
