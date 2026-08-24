#!/usr/bin/env python3
"""Таверна-Лаб: локальний веб-додаток симулятора Hearthstone.

Запуск:  python3 app.py            (відкриє браузер на http://127.0.0.1:8765)
Збірка:  див. build_app.sh (PyInstaller, один виконуваний файл)
"""
import json
import logging
import logging.handlers
import os
import re
import sys
import threading
import time
import uuid
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

if getattr(sys, "frozen", False):          # PyInstaller
    BASE = sys._MEIPASS
    WORKDIR = os.path.dirname(sys.executable)
else:
    BASE = os.path.dirname(os.path.abspath(__file__))
    WORKDIR = BASE
sys.path.insert(0, BASE)
os.chdir(WORKDIR)

from hs2 import carddata, decks
from store import open_store

PORT = 8765
JOBS = {}          # id -> {status, progress:[], result, error}
CACHE_DIR = os.path.join(WORKDIR, "advisor_cache")
os.makedirs(CACHE_DIR, exist_ok=True)
_lock = threading.Lock()
_METAS = None
STORE = None       # set in main(); JOBS is progress-only, sqlite is truth

# Observability: local, cheap, no SaaS. HTTP logging stays off unless
# TAVERNLAB_DEBUG is set, because BaseHTTPRequestHandler logs one line per
# poll and the UI polls jobs at 700 ms.
LOG_PATH = os.path.join(WORKDIR, "tavernlab.log")
DEBUG = bool(os.environ.get("TAVERNLAB_DEBUG"))
log = logging.getLogger("tavernlab")

METRICS = {"games_ingested": 0, "parse_dirty": 0, "reviews_run": 0,
           "review_ms_total": 0.0, "lethal_calls": 0, "errors": 0}

# One review job per game at a time. Startup re-queues every `pending`
# review, so a user who also clicks "Analyse" would otherwise get two
# jobs writing the same generation: `replace_decisions` deletes then
# bulk-inserts, and two interleaved runs trip the UNIQUE key.
_reviewing = {}          # game_id -> job id currently running


def setup_logging():
    if log.handlers:
        return log
    handler = logging.handlers.RotatingFileHandler(
        LOG_PATH, maxBytes=2 * 1024 * 1024, backupCount=3,
        encoding="utf-8")
    handler.setFormatter(logging.Formatter(
        "%(asctime)s %(levelname)s %(name)s %(message)s"))
    log.addHandler(handler)
    log.setLevel(logging.DEBUG if DEBUG else logging.INFO)
    log.propagate = False
    return log


DEFAULT_SETTINGS = {
    "logs_dir": "",
    "language": "",            # "" = follow the browser/OS
    "deckstring": "",
    "player_name": "",
    # Unused until the v1 tailer (PR 6). Kept here so the setting exists
    # and defaults to off before any live code can read it.
    "live_eval": "0",
    "live_lethal_mode": "line",
}


def store():
    """The one Store for this process. Opened lazily so importing app.py
    (tests, tooling) does not create a database as a side effect."""
    global STORE
    with _lock:
        if STORE is None:
            STORE = open_store(WORKDIR)
    return STORE


def metas():
    global _METAS
    with _lock:
        if _METAS is None:
            if not carddata.DEFS:
                carddata.build_defs()
            _METAS = decks.load_meta()
    return _METAS


def deck_key(code):
    import hashlib
    return hashlib.sha1(code.strip().encode()).hexdigest()[:12]


def stats_path(code):
    return os.path.join(CACHE_DIR, deck_key(code) + ".json")


# ------------------------------------------------------------------- jobs
def start_job(fn, *args):
    jid = uuid.uuid4().hex[:10]
    JOBS[jid] = {"status": "running", "progress": [], "result": None,
                 "error": None, "t0": time.time()}

    def run():
        try:
            JOBS[jid]["result"] = fn(jid, *args)
            JOBS[jid]["status"] = "done"
        except Exception as e:
            import traceback
            traceback.print_exc()
            JOBS[jid]["error"] = str(e)
            JOBS[jid]["status"] = "error"
    threading.Thread(target=run, daemon=True).start()
    return jid


def log_to(jid):
    def log(msg):
        JOBS[jid]["progress"].append(str(msg))
    return log


# ------------------------------------------------------- capture / review
def job_import(jid, path, logs_dir=None, only_last=False):
    from capture.hslog_import import import_log
    log_job = log_to(jid)
    log_job(f"Читаю {os.path.basename(path)}…")
    ids = import_log(store(), path, logs_dir=logs_dir,
                     player_name=setting("player_name") or None,
                     deckstring=setting("deckstring") or None,
                     only_last=only_last)
    METRICS["games_ingested"] += len(ids)
    log_job(f"Імпортовано ігор: {len(ids)}")
    logging.getLogger("tavernlab").info(
        "import %s -> %s", os.path.basename(path), ids)
    for gid in ids:
        start_review(gid)
    return {"games": ids, "path": path}


def start_review(game_id, fn=None):
    """Start a review (or a reparse), or hand back the one in flight."""
    st = store()                       # outside the lock: store() takes it
    with _lock:
        jid = _reviewing.get(game_id)
        if jid is not None and JOBS.get(jid, {}).get("status") == "running":
            return jid, False
        _reviewing[game_id] = _PENDING_CLAIM
    st.submit("upsert_review", game_id, "pending")
    jid = start_job(fn or job_review, game_id)
    with _lock:
        _reviewing[game_id] = jid
    return jid, True


_PENDING_CLAIM = "claiming"


def _release_review(game_id):
    with _lock:
        _reviewing.pop(game_id, None)


def job_review(jid, game_id):
    """Post-game review. `reviews.status='pending'` is written *before*
    any work so a crash mid-review is resumable from sqlite."""
    from eval.review import build_review
    st = store()
    log_job = log_to(jid)
    started = time.perf_counter()
    st.submit("upsert_review", game_id, "pending")
    try:
        game = st.get_game(game_id)
        if game is None:
            raise ValueError(f"no such game: {game_id}")
        game = dict(game)
        events = [json.loads(r["payload"]) for r in st.get_events(game_id)]
        if not events:
            st.submit("upsert_review", game_id, "partial",
                      {"error": "no events (legacy games.jsonl import)"})
            return {"game_id": game_id, "status": "partial"}
        log_job(f"Подій: {len(events)}. Реконструюю стан…")
        review, decisions, snapshots = build_review(events, game)
        gen = st.max_generation(game_id) or 1
        # replace, not add: re-analysing the same parse generation is
        # routine and both child tables are uniquely keyed.
        if snapshots:
            st.submit("replace_snapshots", game_id, gen, snapshots)
        if decisions:
            st.submit("replace_decisions", game_id, gen, decisions)
        # `partial` means the review finished but the lethal search hit
        # its budget; the summary is still worth showing, with caveats.
        status = review.get("status", "ready")
        st.submit("upsert_review", game_id, status,
                  {"summary": review,
                   "key_moments": review.get("key_moments"),
                   "evaluator_version": review.get("evaluator_version")})
        log_job("Огляд готовий." if status == "ready"
                else "Огляд частковий (бюджет пошуку вичерпано).")
        METRICS["reviews_run"] += 1
        METRICS["review_ms_total"] += (time.perf_counter() - started) * 1000
        METRICS["lethal_calls"] += sum(1 for d in decisions
                                       if d.get("lethal_ok"))
        logging.getLogger("tavernlab").info(
            "review game=%s status=%s decisions=%s %.0fms",
            game_id, status, len(decisions),
            (time.perf_counter() - started) * 1000)
        return {"game_id": game_id, "status": status,
                "key_moments": len(review.get("key_moments") or [])}
    except Exception as exc:
        METRICS["errors"] += 1
        logging.getLogger("tavernlab").exception(
            "review game=%s failed", game_id)
        st.submit("upsert_review", game_id, "error", {"error": str(exc)})
        raise
    finally:
        _release_review(game_id)


def migrate_legacy_jsonl():
    """One-shot import of the old `games.jsonl` (design §2.5).

    Both prototype schemas land as `mode='unknown'` with no events and
    `reviews.status='partial'`: there is no packet log behind them, so
    they can be listed but never reviewed. Guarded by a setting so the
    next start does not walk the file again.
    """
    st = store()
    path = os.path.join(WORKDIR, "games.jsonl")
    if st.get_setting("jsonl_migrated") or not os.path.exists(path):
        return None
    from store.migrate_jsonl import migrate
    try:
        res = migrate(st, path)
    except Exception as exc:
        print(f"games.jsonl: міграція не вдалася ({exc})")
        return None
    st.submit("set_setting", "jsonl_migrated", str(time.time()))
    return res


def resume_pending_reviews():
    """Startup recovery: in-memory JOBS does not survive a restart."""
    try:
        pending = store().pending_reviews()
    except Exception:
        return []
    for gid in pending:
        start_review(gid)
    return pending


# ------------------------------------------------------------- settings
def setting(key, default=None):
    try:
        return store().get_setting(key, DEFAULT_SETTINGS.get(key, default))
    except Exception:
        return DEFAULT_SETTINGS.get(key, default)


def all_settings():
    out = dict(DEFAULT_SETTINGS)
    try:
        out.update(store().all_settings())
    except Exception:
        pass
    return out


# -------------------------------------------------------------- API logic
def api_resolve(payload):
    from evaluate import try_resolve
    deck, info = try_resolve(payload["code"])
    info["ok"] = deck is not None
    return info


def job_analyze(jid, code, games):
    """Evaluate vs meta + build advisor stats, one pass."""
    from evaluate import try_resolve
    from hs2.sim import gauntlet_winrate
    from hs2.telemetry import build_stats
    from hs2.optimize import deck_counts
    deck, info = try_resolve(code)
    if deck is None:
        raise ValueError(info.get("error") or
                         "Нереалізовані карти: " +
                         ", ".join(info["unimplemented"]))
    log = log_to(jid)
    log(f"Клас: {info['cls'].title()}, {info['total']} карт. Симулюю…")
    avg, rates = gauntlet_winrate(deck, metas(), games)
    log(f"Оцінка готова ({games * len(metas())} боїв). Телеметрія…")
    stats = build_stats(deck, metas(), n_per_opp=max(800, games))
    json.dump({"code": code.strip(), "cls": deck.cls,
               "deck_cards": sorted(deck_counts(deck)),
               "games_per_opp": games, "stats": stats},
              open(stats_path(code), "w"), ensure_ascii=False)
    log("Готово.")
    coach = build_coach(code)
    return {"cls": info["cls"], "cards": info["cards"],
            "avg": round(avg, 4),
            "rates": {k: round(v, 4) for k, v in rates.items()},
            "games": games, "coach": coach}


def job_optimize(jid, code):
    from evaluate import try_resolve
    from hs2.optimize import optimize
    deck, info = try_resolve(code)
    if deck is None:
        raise ValueError("Колода не резолвиться")
    log = log_to(jid)
    best, wr, hist = optimize(deck, metas(), n_eval=250, rounds=2,
                              proposals=12, log=log)
    kept = [(o, i, round(d, 4)) for o, i, w, d in hist if d > 0.015]
    near = sorted([(o, i, round(d, 4)) for o, i, w, d in hist
                   if 0 < d <= 0.015], key=lambda h: -h[2])[:3]
    return {"new_avg": round(wr, 4), "swaps": kept, "near": near}


def build_coach(code):
    data = json.load(open(stats_path(code)))
    stats = data["stats"]
    own = set(data.get("deck_cards", []))
    ranked = sorted(stats.items(),
                    key=lambda kv: kv[1]["wins"] / kv[1]["games"])
    per_card = {}
    for name, s in stats.items():
        base = s["wins"] / s["games"]
        for cn, (on, ow, dn, dw) in s["cards"].items():
            if dn >= 100 and cn in own:
                per_card.setdefault(cn, []).append(dw / dn - base)
    avg = {cn: sum(v) / len(v) for cn, v in per_card.items() if v}
    return {
        "weak": [(n, round(s["wins"] / s["games"], 3))
                 for n, s in ranked[:3]],
        "cut": sorted([(c, round(d, 4)) for c, d in avg.items()],
                      key=lambda x: x[1])[:5],
        "keep": sorted([(c, round(d, 4)) for c, d in avg.items()],
                       key=lambda x: -x[1])[:5],
    }


def api_mull(payload):
    import advisor
    code = payload["code"]
    if not os.path.exists(stats_path(code)):
        return {"error": "Спершу натисніть «Аналізувати колоду»"}
    data = json.load(open(stats_path(code)))
    cls = advisor.CLS_ALIASES.get(payload["opp"].lower(),
                                  payload["opp"].upper())
    opps = [d for d in metas() if d.cls == cls]
    if not opps:
        return {"error": f"Немає мета-колоди класу {cls}"}
    out = []
    for hname in payload["hand"]:
        try:
            card = advisor.find_card(hname.strip())
        except SystemExit as e:
            return {"error": str(e)}
        deltas, ns = [], 0
        for opp in opps:
            s = data["stats"].get(opp.name)
            if not s:
                continue
            base = s["wins"] / s["games"]
            rec = s["cards"].get(card.name)
            if rec and rec[0] >= 30:
                deltas.append(rec[1] / rec[0] - base)
                ns += rec[0]
        delta = sum(deltas) / len(deltas) if deltas else None
        keep = (delta > -0.01) if delta is not None else card.cost <= 3
        out.append({"card": card.name, "cost": card.cost, "keep": keep,
                    "delta": round(delta, 4) if delta is not None else None,
                    "n": ns,
                    "reason": advisor._reason(card, opps[0], delta)})
    s0 = data["stats"].get(opps[0].name)
    return {"opp_deck": opps[0].name,
            "base": round(s0["wins"] / s0["games"], 3) if s0 else None,
            "cards": out}


def api_predict(payload):
    import advisor
    from hs2.optimize import deck_counts
    cls = advisor.CLS_ALIASES.get(payload["opp"].lower(),
                                  payload["opp"].upper())
    opps = [d for d in metas() if d.cls == cls]
    if not opps:
        return {"error": f"Невідомий клас: {payload['opp']}"}
    seen = []
    for s in payload.get("seen", []):
        if s.strip():
            try:
                seen.append(advisor.find_card(s.strip()).name)
            except SystemExit as e:
                return {"error": str(e)}
    out = []
    for opp in opps:
        counts = deck_counts(opp)
        hit = sum(1 for s in seen if s in counts)
        frac = hit / len(seen) if seen else 1.0
        threats = []
        if frac >= 0.5:
            unseen = sorted(
                [(cn, carddata.get_def(cn).cost) for cn in counts
                 if cn not in seen], key=lambda x: -x[1])[:8]
            threats = [{"card": cn, "cost": c,
                        "text": carddata.get_def(cn).text[:90]}
                       for cn, c in unseen]
        out.append({"deck": opp.name, "hits": hit, "seen": len(seen),
                    "frac": round(frac, 2), "threats": threats})
    return {"decks": out}


def api_cardnames(payload):
    """`all=true` returns every collectible name, implemented or not.

    Replay and review must be able to *name* a card the engine cannot
    play; restricting the list to implemented cards would blank out most
    of a real game's board (design PR 12).
    """
    if not carddata.DEFS:
        carddata.build_defs()
    cls = payload.get("cls")
    want_all = bool(payload.get("all"))
    names = sorted({d.name for d in carddata.DEFS.values()
                    if d.coll and (want_all or d.implemented) and
                    (not cls or d.cls in (cls, "NEUTRAL"))})
    return {"names": names, "all": want_all}


def api_cards(payload):
    """Card text/stats by id, for replay hover.

    Deliberately answers for **unimplemented** cards too: a replay has to
    name and describe whatever the opponent actually played, and 78% of
    Standard is not implemented in `hs2` (design §6.2 S3).
    """
    if not carddata.DEFS:
        carddata.build_defs()
    ids = payload.get("ids") or []
    if not isinstance(ids, list):
        return {"error": "ids must be a list"}
    out = {}
    for cid in ids[:400]:
        d = carddata.DEFS.get(cid)
        if d is None:
            continue
        out[cid] = {"name": d.name, "cost": d.cost, "atk": d.atk,
                    "hp": d.hp, "type": d.type, "cls": d.cls,
                    "rarity": d.rarity, "text": d.text,
                    "races": list(d.races),
                    "implemented": bool(d.implemented),
                    "notes": d.notes}
    return {"cards": out}


def api_meta(payload):
    from hs2.optimize import deck_counts
    return {"decks": [{"name": d.name, "cls": d.cls,
                       "cards": sorted(deck_counts(d).items())}
                      for d in metas()]}


def api_winprob(payload):
    from hs2.winprob import winprob_raw
    return {"winprob": round(winprob_raw(payload), 3)}


# ------------------------------------------------------------ new routes
def api_import_log(payload):
    path = (payload.get("path") or "").strip()
    if not path:
        return {"error": "path required"}
    path = os.path.expanduser(path)
    if not os.path.exists(path):
        return {"error": f"not found: {path}"}
    return {"job": start_job(job_import, path,
                             payload.get("logs_dir"),
                             bool(payload.get("only_last")))}


def api_import_last_session(payload):
    from capture.hslog_import import newest_power_log
    logs_dir = (payload.get("logs_dir") or setting("logs_dir") or "")
    logs_dir = os.path.expanduser(logs_dir.strip())
    if not logs_dir:
        return {"error": "logs_dir not set — see Settings"}
    path = newest_power_log(logs_dir)
    if not path:
        return {"error": f"no usable Power.log under {logs_dir}"}
    return {"job": start_job(job_import, path, logs_dir,
                             bool(payload.get("only_last", True))),
            "path": path}


def api_set_settings(payload):
    st = store()
    for key, value in payload.items():
        if key not in DEFAULT_SETTINGS:
            continue
        st.submit("set_setting", key, "" if value is None else str(value))
    return {"settings": all_settings()}


def api_metrics(_payload=None):
    """Local metrics only — no phone-home (design "Observability").

    `pct_search_ok` is expected to read 0.0 in this build; it is reported
    anyway so the number is visible rather than assumed.
    """
    st = store()
    conn = st.read()
    try:
        games = conn.execute("SELECT COUNT(*) c FROM games").fetchone()["c"]
        dirty = conn.execute(
            "SELECT COUNT(*) c FROM games WHERE notes IS NOT NULL"
        ).fetchone()["c"]
        reviews = {r["status"]: r["c"] for r in conn.execute(
            "SELECT status, COUNT(*) c FROM reviews GROUP BY status")}
        row = conn.execute(
            "SELECT COUNT(*) n, SUM(lethal_ok) lo, SUM(search_ok) so "
            "FROM decisions").fetchone()
    finally:
        conn.close()
    n = row["n"] or 0
    runs = METRICS["reviews_run"] or 1
    return {
        "games": games, "parse_dirty": dirty, "reviews": reviews,
        "decisions": n,
        "pct_lethal_ok": round((row["lo"] or 0) / n, 4) if n else None,
        "pct_search_ok": round((row["so"] or 0) / n, 4) if n else None,
        "mean_review_ms": round(METRICS["review_ms_total"] / runs, 1),
        "reviews_run": METRICS["reviews_run"],
        "games_ingested": METRICS["games_ingested"],
        "lethal_calls": METRICS["lethal_calls"],
        "errors": METRICS["errors"],
        "log_path": LOG_PATH, "debug": DEBUG,
    }


def api_labels(_payload=None):
    """The greyed 'coming when calibrated' legend (design §3.3)."""
    from eval import classify as cl
    return {"labels": cl.legend(), "mvp": list(cl.MVP_LABELS),
            "wp_caveat": cl.wp_caveat(),
            "evaluator_version": cl.EVALUATOR_VERSION}


def _qs_int(query, key, default, lo=1, hi=500):
    try:
        return max(lo, min(hi, int((query.get(key) or [default])[0])))
    except (TypeError, ValueError):
        return default


def api_games(query):
    st = store()
    rows = st.list_games(limit=_qs_int(query, "limit", 50),
                         cls=(query.get("class") or [None])[0],
                         result=(query.get("result") or [None])[0])
    # One extra query for every review status, not one per row: the games
    # list is the first screen and each `store.read()` opens its own
    # connection.
    statuses = _review_statuses(st, [r["id"] for r in rows])
    out = []
    for r in rows:
        g = _game_public(r)
        g["review_status"] = statuses.get(r["id"])
        out.append(g)
    return {"games": out}


def _review_statuses(st, game_ids):
    if not game_ids:
        return {}
    marks = ",".join("?" * len(game_ids))
    conn = st.read()
    try:
        cur = conn.execute(
            f"SELECT game_id, status FROM reviews WHERE game_id IN "
            f"({marks})", tuple(game_ids))
        return {r["game_id"]: r["status"] for r in cur.fetchall()}
    finally:
        conn.close()


def api_game(game_id):
    row = store().get_game(game_id)
    if row is None:
        return {"error": "not found"}, 404
    return {"game": _game_public(row)}


# Constructed only. Arena and Battlegrounds have different logs, ratings
# and engines; the design keeps them as extension seams, not product
# (Q6: tag the mode, disable review with a reason).
UNREVIEWABLE_MODES = ("arena", "bg")


def _game_public(row):
    """`games` row minus the gzip blob — it is megabytes."""
    out = {k: row[k] for k in row.keys() if k != "raw_power"}
    out["reviewable"], out["review_blocked"] = _reviewable(out)
    return out


def _reviewable(game):
    """Can this game be reviewed, and if not, say why in one phrase."""
    mode = (game.get("mode") or "unknown").lower()
    if any(mode.startswith(m) for m in UNREVIEWABLE_MODES):
        return False, f"{mode} is not constructed"
    if not game.get("log_hash"):
        return False, "imported before packet logs (games.jsonl)"
    return True, None


def api_game_events(game_id, query):
    from_seq = _qs_int(query, "from_seq", 0, lo=0, hi=10 ** 9)
    rows = store().get_events(game_id, from_seq=from_seq)
    return {"events": [{"seq": r["seq"], "type": r["type"],
                        "ts": r["ts_log"],
                        "payload": json.loads(r["payload"])}
                       for r in rows]}


def api_game_replay(game_id):
    """Snapshots, compact: the UI scrubs these, it does not re-reduce."""
    rows = store().get_snapshots(game_id)
    return {"snapshots": [{"event_seq": r["event_seq"],
                           "visible": json.loads(r["visible"]),
                           "lethal_ok": r["lethal_ok"],
                           "search_ok": r["search_ok"],
                           "wp": r["wp"], "wp_source": r["wp_source"],
                           "unimplemented": json.loads(
                               r["unimplemented"] or "[]")}
                          for r in rows]}


def api_game_review(game_id):
    st = store()
    rev = st.get_review(game_id)
    if rev is None:
        return {"error": "no review", "status": None}, 404
    if not rev["summary"]:
        # pending / error / a legacy games.jsonl row with no packets
        return {"game_id": game_id, "status": rev["status"],
                "error": rev["error"]}
    out = json.loads(rev["summary"])
    out["status"] = rev["status"]
    out["game_id"] = game_id
    return out


def api_game_analyze(game_id):
    # `pending` lands before the job starts, so a restart resumes it.
    jid, started = start_review(game_id)
    return {"job": jid, "already_running": not started}


def job_reparse(jid, game_id):
    """Re-run the importer over the stored Power.log slice (risk R1).

    The point of keeping `games.raw_power` is that a card whose packets we
    mis-parsed today can be recovered after an importer fix without the
    user still having the log.  The new parse lands at generation+1 and
    every read switches to it, because the generation rule always takes
    MAX (design §2.2).
    """
    import gzip as _gzip
    import tempfile
    from capture.hslog_import import (PARSE_GENERATION, event_rows,
                                      parsed_games)
    st = store()
    say = log_to(jid)
    row = st.get_game(game_id)
    if row is None or not row["raw_power"]:
        raise ValueError("no stored Power.log slice for this game")
    gen = max(st.max_generation(game_id) or PARSE_GENERATION,
              PARSE_GENERATION) + 1
    blob = _gzip.decompress(row["raw_power"]).decode("utf-8", "replace")
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "Power.log")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(blob)
        parsed = list(parsed_games(path))
    if not parsed:
        raise ValueError("the stored slice no longer parses")
    _raw, events, summ, _p = parsed[0]
    say(f"Перерозбір: {len(events)} подій (покоління {gen}).")
    st.submit("add_events", game_id, gen, event_rows(events))
    st.submit("update_game", game_id,
              {"turns": summ.turns, "result": summ.result_for(
                  row["player_id"] or 1)})
    logging.getLogger("tavernlab").info(
        "reparse game=%s generation=%s events=%s", game_id, gen,
        len(events))
    # Runs inline; its `finally` releases the claim this job took.
    return job_review(jid, game_id)


def api_game_reparse(game_id):
    jid, started = start_review(game_id, job_reparse)
    return {"job": jid, "already_running": not started}


ROUTES = {"/api/resolve": api_resolve, "/api/mull": api_mull,
          "/api/predict": api_predict, "/api/meta": api_meta,
          "/api/cardnames": api_cardnames, "/api/cards": api_cards,
          "/api/winprob": api_winprob,
          "/api/import/log": api_import_log,
          "/api/import/last_session": api_import_last_session,
          "/api/settings": api_set_settings}

# GET routes. `webui.html`'s `api()` is POST-only, so the UI needs a
# separate `getJSON()` for every one of these (design §2.7).
_GAME_RE = re.compile(r"^/api/games/(\d+)$")
_GAME_SUB_RE = re.compile(r"^/api/games/(\d+)/(events|replay|review)$")
_ANALYZE_RE = re.compile(r"^/api/games/(\d+)/(analyze|reparse)$")
_LOCALE_RE = re.compile(r"^/locales/([a-z]{2}(?:-[A-Za-z]{2})?)\.json$")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        """Stop swallowing HTTP logs — but only under the debug flag.

        The UI polls `/api/job/` every 700 ms, so unconditional access
        logging would bury everything else in the rotating file.
        """
        if DEBUG:
            log.debug("%s %s", self.address_string(), fmt % args)

    def _send(self, obj, code=200, ctype="application/json"):
        body = obj if isinstance(obj, bytes) else \
            json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype + "; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _reply(self, out):
        """Handlers may return `obj` or `(obj, status)`."""
        if isinstance(out, tuple):
            return self._send(out[0], out[1])
        return self._send(out)

    def do_GET(self):
        parsed = urlparse(self.path)
        path, query = parsed.path, parse_qs(parsed.query)
        if path in ("/", "/index.html"):
            html = open(os.path.join(BASE, "webui.html"), "rb").read()
            return self._send(html, ctype="text/html")
        m = _LOCALE_RE.match(path)
        if m:
            return self._send_locale(m.group(1))
        if path.startswith("/api/job/"):
            jid = path.rsplit("/", 1)[1]
            job = JOBS.get(jid)
            if not job:
                return self._send({"error": "no such job"}, 404)
            return self._send({k: job[k] for k in
                               ("status", "progress", "result", "error")})
        try:
            if path == "/api/games":
                return self._reply(api_games(query))
            if path == "/api/settings":
                return self._send({"settings": all_settings()})
            if path == "/api/labels":
                return self._send(api_labels())
            if path == "/api/metrics":
                return self._send(api_metrics())
            m = _GAME_SUB_RE.match(path)
            if m:
                gid, sub = int(m.group(1)), m.group(2)
                if sub == "events":
                    return self._reply(api_game_events(gid, query))
                if sub == "replay":
                    return self._reply(api_game_replay(gid))
                return self._reply(api_game_review(gid))
            m = _GAME_RE.match(path)
            if m:
                return self._reply(api_game(int(m.group(1))))
        except Exception as e:
            import traceback
            traceback.print_exc()
            return self._send({"error": str(e)}, 500)
        return self._send({"error": "not found"}, 404)

    def _send_locale(self, lang):
        path = os.path.join(BASE, "locales", f"{lang}.json")
        if not os.path.exists(path):
            return self._send({"error": "no such locale"}, 404)
        return self._send(open(path, "rb").read())

    def do_POST(self):
        path = urlparse(self.path).path
        n = int(self.headers.get("Content-Length", 0))
        payload = json.loads(self.rfile.read(n) or b"{}")
        if path == "/api/analyze":
            jid = start_job(job_analyze, payload["code"],
                            int(payload.get("games", 1000)))
            return self._send({"job": jid})
        if path == "/api/optimize":
            jid = start_job(job_optimize, payload["code"])
            return self._send({"job": jid})
        m = _ANALYZE_RE.match(path)
        if m:
            try:
                gid = int(m.group(1))
                if m.group(2) == "reparse":
                    return self._reply(api_game_reparse(gid))
                return self._reply(api_game_analyze(gid))
            except Exception as e:
                return self._send({"error": str(e)}, 500)
        fn = ROUTES.get(path)
        if fn is None:
            return self._send({"error": "not found"}, 404)
        try:
            return self._reply(fn(payload))
        except Exception as e:
            import traceback
            traceback.print_exc()
            return self._send({"error": str(e)}, 500)


def main():
    setup_logging()
    log.info("start workdir=%s debug=%s", WORKDIR, DEBUG)
    print("Таверна-Лаб: завантажую карти…")
    metas()
    store()
    moved = migrate_legacy_jsonl()
    if moved:
        print(f"games.jsonl → SQLite: {moved.get('imported', 0)} записів "
              f"(перегляду немає — старий формат без пакетів)")
    resumed = resume_pending_reviews()
    if resumed:
        print(f"Відновлюю незавершені огляди: {len(resumed)}")
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    url = f"http://127.0.0.1:{PORT}"
    print(f"Готово: {url}  (Ctrl+C — вихід)")
    threading.Timer(0.6, lambda: webbrowser.open(url)).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    import multiprocessing
    multiprocessing.freeze_support()
    main()
