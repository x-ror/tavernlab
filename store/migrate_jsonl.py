"""One-shot import of the legacy `games.jsonl` into sqlite.

Two different writers appended to that file and neither knew about the
other, so a line is in one of two shapes:

  live.py     {"ts", "players": {"1": name}, "classes": {"1": "MAGE"},
               "played": {...}, "result": {"winner": name|None}, "turns"}
  watcher.py  {"ts", "players": [{"name", "won"}], "plays": [[pid, id]]}

Neither carries a packet log, so the games land with no events and
`reviews.status='partial'` -- enough for the library list and the win
counters, honestly marked as un-reviewable.  Re-running is safe: the
sha1 of the raw line is stored in `games.log_hash` and already-imported
lines are skipped.
"""
import argparse
import hashlib
import json
import os
import sys

NOTE = "legacy games.jsonl import (%s schema); no packet log"


def _result(won_us, won_them):
    if won_us:
        return "win"
    if won_them:
        return "loss"
    return "unknown"


def _from_watcher(rec):
    """`players` is a list of {name, won}; index 0 is treated as us."""
    players = rec.get("players") or []
    us = players[0] if len(players) > 0 else {}
    them = players[1] if len(players) > 1 else {}
    return {
        "player_name": us.get("name"),
        "player_id": 1,
        "opponent_name": them.get("name"),
        "result": _result(us.get("won"), them.get("won")),
        "notes": NOTE % "watcher",
    }


def _from_live(rec):
    """`players`/`classes` are keyed by log player id; 1 is us."""
    players = rec.get("players") or {}
    classes = rec.get("classes") or {}
    winner = (rec.get("result") or {}).get("winner")
    us, them = players.get("1"), players.get("2")
    return {
        "player_name": us,
        "player_id": 1,
        "player_class": classes.get("1"),
        "opponent_name": them,
        "opponent_class": classes.get("2"),
        "turns": rec.get("turns"),
        "result": _result(winner is not None and winner == us,
                          winner is not None and winner == them),
        "notes": NOTE % "live",
    }


def to_game(rec, log_hash):
    """Map one legacy record onto a `games` row."""
    if isinstance(rec.get("players"), list) or "plays" in rec:
        row = _from_watcher(rec)
    else:
        row = _from_live(rec)
    row["started_at"] = rec.get("ts") or 0.0
    row["mode"] = "unknown"
    row["log_hash"] = log_hash
    return {k: v for k, v in row.items() if v is not None}


def migrate(store, path):
    """Import every unseen line of `path`.  Returns counts."""
    if not os.path.exists(path):
        return {"imported": 0, "skipped": 0}
    conn = store.read()
    try:
        seen = {r["log_hash"] for r in conn.execute(
            "SELECT log_hash FROM games WHERE log_hash IS NOT NULL")}
    finally:
        conn.close()
    imported = skipped = 0
    with open(path, encoding="utf-8", errors="replace") as f:
        for raw in f:
            raw = raw.strip()
            if not raw:
                continue
            log_hash = hashlib.sha1(raw.encode("utf-8")).hexdigest()
            if log_hash in seen:
                skipped += 1
                continue
            try:
                rec = json.loads(raw)
            except ValueError:
                skipped += 1
                continue
            if not isinstance(rec, dict):
                skipped += 1
                continue
            game_id = store.submit("create_game", to_game(rec, log_hash))
            # 'partial' and not 'pending': there is nothing to review, so
            # startup resume must not pick these up.
            store.submit("upsert_review", game_id, "partial")
            seen.add(log_hash)
            imported += 1
    return {"imported": imported, "skipped": skipped}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--jsonl", default="games.jsonl")
    ap.add_argument("--workdir", default=".",
                    help="directory holding tavernlab.sqlite")
    args = ap.parse_args(argv)
    from store.db import open_store
    store = open_store(args.workdir)
    try:
        stats = migrate(store, args.jsonl)
    finally:
        store.close()
    print("imported %(imported)d, skipped %(skipped)d" % stats)
    return 0


if __name__ == "__main__":
    sys.path.insert(0, os.path.dirname(os.path.dirname(
        os.path.abspath(__file__))))
    sys.exit(main())
