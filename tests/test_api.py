"""PR 11 (server half) + PR 12: the new HTTP surface.

Run against a real `ThreadingHTTPServer` on an ephemeral port, because
the bug class this guards is routing, not logic: `webui.html`'s `api()`
is POST-only, every new read route is a GET, and a GET that falls through
to the 404 branch looks exactly like an empty result in the UI.
"""
import json
import logging
import os
import threading
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer

import pytest

import app
from capture.hslog_import import import_log
from store import Store

FIX = os.path.join(os.path.dirname(os.path.abspath(__file__)), "logs",
                   "fixtures")


@pytest.fixture(scope="module")
def server(tmp_path_factory, carddb):
    """A live app with a throwaway store holding two reviewed games."""
    logging.disable(logging.WARNING)
    tmp = tmp_path_factory.mktemp("api")
    st = Store(str(tmp / "t.sqlite"))
    ids = []
    for name in ("synthetic_missed_lethal.log", "real_game1.log.gz"):
        ids += import_log(st, os.path.join(FIX, name))
    prev_store, app.STORE = app.STORE, st
    # `job_review` logs progress through JOBS; register the id the way
    # `start_job` would rather than spawning a thread we then have to
    # wait on.
    app.JOBS["fixture"] = {"status": "running", "progress": [],
                           "result": None, "error": None, "t0": 0}
    for gid in ids:
        app.job_review("fixture", gid)

    srv = ThreadingHTTPServer(("127.0.0.1", 0), app.Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    try:
        yield base, ids, st
    finally:
        srv.shutdown()
        st.close()
        app.STORE = prev_store
        logging.disable(logging.NOTSET)


def get(base, path):
    with urllib.request.urlopen(base + path, timeout=10) as r:
        return json.loads(r.read())


def get_status(base, path):
    try:
        urllib.request.urlopen(base + path, timeout=10)
        return 200
    except urllib.error.HTTPError as e:
        return e.code


def post(base, path, body=None):
    req = urllib.request.Request(
        base + path, data=json.dumps(body or {}).encode(),
        headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read())


# ------------------------------------------------------------- existing
def test_legacy_post_routes_still_work(server):
    base, _ids, _st = server
    assert post(base, "/api/meta")["decks"]
    assert 0.0 < post(base, "/api/winprob", {"my_turn": 5})["winprob"] < 1
    assert post(base, "/api/cardnames", {})["names"]


def test_index_still_serves_the_ui(server):
    base, _ids, _st = server
    with urllib.request.urlopen(base + "/", timeout=10) as r:
        body = r.read().decode()
    assert r.headers["Content-Type"].startswith("text/html")
    assert "<script>" in body


# ------------------------------------------------------------ new reads
def test_games_list(server):
    base, ids, _st = server
    games = get(base, "/api/games")["games"]
    assert len(games) == len(ids)
    g = games[0]
    for key in ("id", "player_class", "opponent_class", "result", "turns",
                "review_status"):
        assert key in g
    assert g["review_status"] in ("ready", "partial", "pending", "error")
    assert "raw_power" not in g, "the gzip blob must not ship to the UI"


def test_games_list_filters(server):
    base, _ids, _st = server
    losses = get(base, "/api/games?result=loss")["games"]
    assert losses and all(g["result"] == "loss" for g in losses)
    assert get(base, "/api/games?result=tie")["games"] == []
    assert len(get(base, "/api/games?limit=1")["games"]) == 1


def test_single_game_and_404(server):
    base, ids, _st = server
    g = get(base, f"/api/games/{ids[0]}")["game"]
    assert g["id"] == ids[0]
    assert get_status(base, "/api/games/99999") == 404


def test_review_route_returns_the_ui_contract(server):
    base, ids, _st = server
    rev = get(base, f"/api/games/{ids[0]}/review")
    assert rev["status"] == "ready"
    for key in ("wp_series", "key_moments", "turns", "report",
                "labels_legend", "evaluator_version"):
        assert key in rev
    assert all(p["hatch"] for p in rev["wp_series"])
    assert "9 weights" in " ".join(rev["report"]["caveats"])


def test_missed_lethal_survives_the_store_round_trip(server):
    base, ids, _st = server
    rev = get(base, f"/api/games/{ids[0]}/review")
    moments = [m for m in rev["key_moments"]
               if m["label"] == "missed_lethal"]
    assert moments and moments[0]["turn"] == 7
    assert "Fireball" in moments[0]["detail"]


def test_events_route_paginates_from_seq(server):
    base, ids, _st = server
    all_ev = get(base, f"/api/games/{ids[0]}/events")["events"]
    assert all_ev and all_ev[0]["type"] == "CREATE_GAME"
    tail = get(base, f"/api/games/{ids[0]}/events?from_seq=5")["events"]
    assert len(tail) == len(all_ev) - 5
    assert tail[0]["seq"] == 5
    assert isinstance(tail[0]["payload"], dict)


def test_replay_route_serves_snapshots(server):
    base, ids, _st = server
    body = get(base, f"/api/games/{ids[0]}/replay")
    snaps = body["snapshots"]
    assert snaps, "the replay scrubber would have nothing to scrub"
    seqs = [s["event_seq"] for s in snaps]
    assert seqs == sorted(seqs)
    for snap in snaps:
        assert snap["search_ok"] == 0, "MVP invariant"
        assert isinstance(snap["visible"], dict)
        assert snap["wp_source"] == "logistic_v1"
        assert 0.0 <= snap["wp"] <= 1.0
        vs = snap["visible"]
        assert "boards" in vs and "hands" in vs and "heroes" in vs


def test_replay_seqs_line_up_with_the_review_series(server):
    base, ids, _st = server
    snaps = get(base, f"/api/games/{ids[0]}/replay")["snapshots"]
    series = get(base, f"/api/games/{ids[0]}/review")["wp_series"]
    assert [s["event_seq"] for s in snaps] == [p["seq"] for p in series]
    assert [s["wp"] for s in snaps] == [p["wp"] for p in series]


def test_labels_route_is_the_greyed_legend(server):
    base, _ids, _st = server
    body = get(base, "/api/labels")
    by_key = {e["key"]: e for e in body["labels"]}
    assert by_key["missed_lethal"]["available"] is True
    assert by_key["blunder"]["available"] is False
    assert set(body["mvp"]) == {"missed_lethal", "played_lethal", "note"}
    assert "hatched" in body["wp_caveat"]


# --------------------------------------------------------------- writes
def test_settings_round_trip_and_reject_unknown_keys(server):
    base, _ids, _st = server
    before = get(base, "/api/settings")["settings"]
    assert before["live_eval"] == "0", "live eval must default to off"
    assert before["live_lethal_mode"] == "line"
    out = post(base, "/api/settings",
               {"logs_dir": "/tmp/logs", "language": "uk",
                "nonsense": "x"})["settings"]
    assert out["logs_dir"] == "/tmp/logs" and out["language"] == "uk"
    assert "nonsense" not in out
    assert get(base, "/api/settings")["settings"]["logs_dir"] == "/tmp/logs"


def test_import_rejects_a_missing_path(server):
    base, _ids, _st = server
    assert "error" in post(base, "/api/import/log", {"path": "/nope.log"})
    assert "error" in post(base, "/api/import/log", {})


def test_import_last_session_needs_a_logs_dir(server, tmp_path):
    base, _ids, _st = server
    post(base, "/api/settings", {"logs_dir": ""})
    assert "error" in post(base, "/api/import/last_session", {})
    assert "error" in post(base, "/api/import/last_session",
                           {"logs_dir": str(tmp_path)})


def test_import_a_fixture_end_to_end(server):
    base, ids, _st = server
    before = len(get(base, "/api/games")["games"])
    res = post(base, "/api/import/log",
               {"path": os.path.join(FIX, "synthetic_fireball.log")})
    assert "job" in res
    job = _await(base, res["job"])
    assert job["status"] == "done", job.get("error")
    assert len(job["result"]["games"]) == 1
    assert len(get(base, "/api/games")["games"]) == before + 1


def test_reanalyse_marks_pending_then_ready(server):
    base, ids, st = server
    res = post(base, f"/api/games/{ids[0]}/analyze")
    assert "job" in res
    job = _await(base, res["job"])
    assert job["status"] == "done", job.get("error")
    assert st.get_review(ids[0])["status"] == "ready"


def test_cardnames_all_includes_unimplemented(server):
    base, _ids, _st = server
    impl = post(base, "/api/cardnames", {})["names"]
    every = post(base, "/api/cardnames", {"all": True})
    assert every["all"] is True
    assert len(every["names"]) > len(impl)
    assert set(impl) <= set(every["names"])


def test_unknown_routes_still_404(server):
    base, _ids, _st = server
    assert get_status(base, "/api/nope") == 404
    assert get_status(base, "/locales/zz.json") == 404


def _await(base, jid, tries=600):
    import time
    for _ in range(tries):
        job = get(base, f"/api/job/{jid}")
        if job["status"] != "running":
            return job
        time.sleep(0.05)
    raise AssertionError("job never finished")


def test_query_params_are_hardened(server):
    """A junk `?limit=` in a bookmarked URL must not 500 the games list."""
    base, ids, _st = server
    for bad in ("limit=abc", "limit=-3", "limit=99999", "limit="):
        body = get(base, "/api/games?" + bad)
        assert "games" in body, bad
    assert get_status(base, "/api/games?limit=abc") == 200
    ev = get(base, f"/api/games/{ids[0]}/events?from_seq=nope")["events"]
    assert ev and ev[0]["seq"] == 0


def test_games_list_does_not_query_per_row(server):
    base, ids, st = server
    calls = []
    real_read = st.read

    def counting_read():
        calls.append(1)
        return real_read()

    st.read = counting_read
    try:
        games = get(base, "/api/games")["games"]
    finally:
        st.read = real_read
    assert len(games) >= 2
    assert len(calls) <= 3, (
        f"{len(calls)} connections for {len(games)} games — the review "
        f"status lookup is per-row again")
    assert all(g["review_status"] for g in games)


def test_games_carry_a_reviewable_flag_with_a_reason(server):
    """Q6: tag the mode and disable review with a reason, rather than
    letting the button fail silently on an Arena run."""
    base, _ids, st = server
    games = get(base, "/api/games")["games"]
    assert all(g["reviewable"] for g in games)
    assert all(g["review_blocked"] is None for g in games)

    arena = st.submit("create_game", {"mode": "arena", "log_hash": "x",
                                      "created_at": 0, "started_at": 0})
    legacy = st.submit("create_game", {"mode": "unknown",
                                       "created_at": 0, "started_at": 0})
    by_id = {g["id"]: g for g in get(base, "/api/games")["games"]}
    assert by_id[arena]["reviewable"] is False
    assert "constructed" in by_id[arena]["review_blocked"]
    assert by_id[legacy]["reviewable"] is False
    assert "games.jsonl" in by_id[legacy]["review_blocked"]


def test_cards_route_describes_unimplemented_cards_too(server):
    """A replay has to name whatever the opponent played, and most of
    Standard is not implemented in `hs2` (design §6.2 S3)."""
    from hs2 import carddata
    base, _ids, _st = server
    if not carddata.DEFS:
        carddata.build_defs()
    missing = next(d for d in carddata.DEFS.values()
                   if d.coll and not d.implemented)
    known = carddata.get_def("Fireball")
    body = post(base, "/api/cards", {"ids": [missing.id, known.id,
                                             "NOT_A_CARD"]})["cards"]
    assert set(body) == {missing.id, known.id}
    assert body[missing.id]["implemented"] is False
    assert body[missing.id]["name"] == missing.name
    assert body[known.id]["cost"] == 4 and body[known.id]["text"]
    assert "error" in post(base, "/api/cards", {"ids": "nope"})
    assert post(base, "/api/cards", {})["cards"] == {}


# ------------------------------------------------------- observability
def test_metrics_are_local_and_report_the_mvp_invariant(server):
    """Design "Observability": local JSON, no phone-home. `search_ok`
    is reported rather than assumed to be zero."""
    base, _ids, _st = server
    m = get(base, "/api/metrics")
    assert m["games"] >= 2 and m["decisions"] > 0
    assert m["reviews"]["ready"] >= 1
    assert m["pct_search_ok"] == 0.0, "MVP invariant is measured, not assumed"
    assert 0.0 <= m["pct_lethal_ok"] <= 1.0
    assert m["mean_review_ms"] > 0 and m["reviews_run"] >= 1
    assert m["log_path"].endswith("tavernlab.log")
    assert set(m) >= {"games", "parse_dirty", "reviews", "mean_review_ms",
                      "lethal_calls", "errors", "debug"}


def test_reparse_rebuilds_events_at_a_new_generation(server):
    """Risk R1: an importer fix has to be recoverable from the stored
    slice, without the user still having the original log."""
    base, ids, st = server
    gid = ids[0]
    before = st.get_events(gid)
    gen_before = st.max_generation(gid)
    job = _await(base, post(base, f"/api/games/{gid}/reparse")["job"])
    assert job["status"] == "done", job.get("error")

    gen_after = st.max_generation(gid)
    assert gen_after == gen_before + 1
    after = st.get_events(gid)
    # The generation rule means reads switch to the new parse wholesale.
    assert all(e["parse_generation"] == gen_after for e in after)
    assert [e["type"] for e in after] == [e["type"] for e in before]
    assert st.get_review(gid)["status"] in ("ready", "partial")
    # Decisions and snapshots follow the events to the new generation.
    assert all(d["parse_generation"] == gen_after
               for d in st.get_decisions(gid))
    assert all(s["parse_generation"] == gen_after
               for s in st.get_snapshots(gid))


def test_reparse_without_a_stored_slice_fails_loudly(server, tmp_path):
    base, _ids, st = server
    gid = st.submit("create_game", {"mode": "casual", "created_at": 0,
                                    "started_at": 0, "log_hash": "none"})
    job = _await(base, post(base, f"/api/games/{gid}/reparse")["job"])
    assert job["status"] == "error"
    assert "raw_power" in job["error"] or "slice" in job["error"]
