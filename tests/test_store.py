"""Store contract: schema bootstrap, the writer queue, the generation
rule, and the legacy jsonl migrator."""
import json
import os
import sqlite3
import threading

import pytest

from store import open_store
from store.db import Store
from store.migrate_jsonl import migrate

LIVE_LINE = {
    "ts": 1787572892.14,
    "players": {"1": "Andrij", "2": "EnemyRogue"},
    "classes": {"1": "MAGE", "2": "ROGUE"},
    "played": {"1": ["First Flame"], "2": ["Preparation"]},
    "result": {"winner": "Andrij"},
    "turns": 2,
}
WATCHER_LINE = {
    "ts": 1787574015.07,
    "players": [{"name": "Andrij", "won": False},
                {"name": "EnemyPriest", "won": True}],
    "plays": [[1, "EX1_277"], [2, "CS2_004"]],
}


@pytest.fixture
def store(tmp_path):
    s = Store(str(tmp_path / "tavernlab.sqlite"))
    yield s
    s.close()


def mkgame(store, **kw):
    kw.setdefault("started_at", 1000.0)
    return store.submit("create_game", kw)


def ev(seq, type="TAG_CHANGE", **kw):
    row = {"seq": seq, "type": type, "payload": {"seq": seq}}
    row.update(kw)
    return row


# -- schema -------------------------------------------------------------

def test_schema_applies_once_and_reopen_is_idempotent(tmp_path):
    path = tmp_path / "tavernlab.sqlite"
    s = Store(str(path))
    gid = mkgame(s, player_class="MAGE")
    s.close()
    assert path.exists()

    s2 = Store(str(path))
    try:
        conn = s2.read()
        try:
            rows = conn.execute(
                "SELECT version FROM schema_migrations").fetchall()
            tables = {r["name"] for r in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table'")}
            mode = conn.execute("PRAGMA journal_mode").fetchone()[0]
        finally:
            conn.close()
        assert [r["version"] for r in rows] == [1]
        assert {"games", "events", "snapshots", "decisions", "reviews",
                "decks", "cards", "meta_decks", "sources", "settings",
                "schema_migrations"} <= tables
        assert mode == "wal"
        assert s2.get_game(gid)["player_class"] == "MAGE"
    finally:
        s2.close()


def test_open_store_uses_workdir_filename(tmp_path):
    s = open_store(str(tmp_path))
    try:
        assert s.path == str(tmp_path / "tavernlab.sqlite")
    finally:
        s.close()
    assert (tmp_path / "tavernlab.sqlite").exists()


# -- writer queue -------------------------------------------------------

def test_concurrent_writes_from_eight_threads_all_land(store):
    errors = []
    per_thread = 12

    def work(n):
        try:
            for i in range(per_thread):
                gid = store.submit("create_game", {
                    "started_at": 1000.0 + n,
                    "player_name": "t%d" % n,
                    "mode": "unknown"})
                store.submit("add_events", gid, 1,
                             [ev(0, "GAME_START"), ev(1)])
                store.submit("upsert_review", gid, "pending")
                assert store.get_game(gid) is not None
        except Exception as e:                      # noqa: BLE001
            errors.append(e)

    threads = [threading.Thread(target=work, args=(n,)) for n in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=60)

    assert errors == []
    conn = store.read()
    try:
        games = conn.execute("SELECT COUNT(*) FROM games").fetchone()[0]
        events = conn.execute("SELECT COUNT(*) FROM events").fetchone()[0]
    finally:
        conn.close()
    assert games == 8 * per_thread
    assert events == 8 * per_thread * 2
    assert len(store.pending_reviews()) == 8 * per_thread


def test_reader_sees_committed_writes_immediately(store):
    gid = mkgame(store, player_class="PRIEST")
    assert store.get_game(gid)["player_class"] == "PRIEST"
    store.submit("update_game", gid, {"result": "win", "turns": 9})
    row = store.get_game(gid)
    assert (row["result"], row["turns"]) == ("win", 9)

    store.submit("set_raw_power", gid, b"\x1f\x8bgzipped")
    assert bytes(store.get_game(gid)["raw_power"]) == b"\x1f\x8bgzipped"

    store.submit("set_setting", "logs_dir", "/tmp/logs")
    store.submit("set_setting", "live_eval", True)
    assert store.get_setting("logs_dir") == "/tmp/logs"
    assert store.get_setting("missing", "dflt") == "dflt"
    assert store.all_settings() == {"logs_dir": "/tmp/logs",
                                    "live_eval": "true"}


def test_read_returns_a_fresh_connection_each_call(store):
    a, b = store.read(), store.read()
    try:
        assert a is not b
    finally:
        a.close()
        b.close()


def test_writer_exception_is_reraised_in_caller(store):
    with pytest.raises(ValueError):
        store.submit("create_game", {"no_such_column": 1})
    with pytest.raises(AttributeError):
        store.submit("no_such_op", 1)
    # the writer thread survived and still serves later ops
    assert mkgame(store) > 0


def test_submit_after_close_raises(tmp_path):
    s = Store(str(tmp_path / "tavernlab.sqlite"))
    s.close()
    with pytest.raises(RuntimeError):
        s.submit("create_game", {})


def test_add_events_respects_unique_seq_constraint(store):
    gid = mkgame(store)
    assert store.submit("add_events", gid, 1,
                        [ev(0), ev(1), ev(2)]) == 3
    with pytest.raises(sqlite3.IntegrityError):
        store.submit("add_events", gid, 1, [ev(3), ev(1)])
    # the whole batch rolled back, so seq 3 did not sneak in
    assert [r["seq"] for r in store.get_events(gid)] == [0, 1, 2]
    # same seq under a different generation is fine
    store.submit("add_events", gid, 2, [ev(0), ev(1)])
    assert len(store.get_events(gid)) == 2


def test_add_events_numbers_seq_from_enumerate_when_absent(store):
    gid = mkgame(store)
    rows = [{"type": "A", "payload": {}}, {"type": "B", "payload": {}}]
    store.submit("add_events", gid, 1, rows)
    assert [r["seq"] for r in store.get_events(gid)] == [0, 1]
    assert json.loads(store.get_events(gid)[0]["payload"]) == {}


# -- generation rule ----------------------------------------------------

def snap(seq, **kw):
    row = {"event_seq": seq, "visible": {"turn": seq}}
    row.update(kw)
    return row


def dec(seq, kind="play", **kw):
    row = {"event_seq": seq, "side": "us", "kind": kind,
           "chosen": {"card": "Fireball"}, "explanation": {"what": "x"},
           "evaluator_version": "eval-0.1.0+hs2"}
    row.update(kw)
    return row


def test_reads_return_only_the_max_generation(store):
    gid = mkgame(store)
    store.submit("add_events", gid, 0, [ev(0, "tail"), ev(1, "tail")])
    store.submit("add_snapshots", gid, 0, [snap(0)])
    store.submit("add_decisions", gid, 0, [dec(0)])
    store.submit("add_events", gid, 1,
                 [ev(0, "GAME_START"), ev(1), ev(2)])
    store.submit("add_snapshots", gid, 1, [snap(0), snap(2)])
    store.submit("add_decisions", gid, 1, [dec(0), dec(2, "attack")])

    events = store.get_events(gid)
    assert [e["type"] for e in events] == ["GAME_START", "TAG_CHANGE",
                                           "TAG_CHANGE"]
    assert {e["parse_generation"] for e in events} == {1}
    assert {s["parse_generation"] for s in store.get_snapshots(gid)} == {1}
    assert {d["parse_generation"] for d in store.get_decisions(gid)} == {1}
    assert len(store.get_snapshots(gid)) == 2
    assert len(store.get_decisions(gid)) == 2
    assert store.max_generation(gid) == 1
    assert [e["seq"] for e in store.get_events(gid, from_seq=1)] == [1, 2]


def test_generation_zero_is_never_returned(store):
    gid = mkgame(store)
    store.submit("add_events", gid, 0, [ev(0, "tail"), ev(1, "tail")])
    store.submit("add_snapshots", gid, 0, [snap(0)])
    store.submit("add_decisions", gid, 0, [dec(0)])
    # an in-progress tail is invisible to every read helper
    assert store.get_events(gid) == []
    assert store.get_snapshots(gid) == []
    assert store.get_decisions(gid) == []
    # ...but the rows are really there, and gen 0 is a real generation
    assert store.max_generation(gid) == 0
    conn = store.read()
    try:
        n = conn.execute("SELECT COUNT(*) FROM events WHERE game_id=?",
                         (gid,)).fetchone()[0]
    finally:
        conn.close()
    assert n == 2


def test_generation_filter_is_per_game(store):
    a, b = mkgame(store), mkgame(store)
    store.submit("add_events", a, 1, [ev(0), ev(1)])
    store.submit("add_events", b, 0, [ev(0, "tail")])
    assert len(store.get_events(a)) == 2
    assert store.get_events(b) == []
    assert store.max_generation(mkgame(store)) is None


def test_delete_generation_drops_gen0_and_keeps_gen1(store):
    gid = mkgame(store)
    store.submit("add_events", gid, 0, [ev(0, "tail"), ev(1, "tail")])
    store.submit("add_snapshots", gid, 0, [snap(0)])
    store.submit("add_decisions", gid, 0, [dec(0)])
    store.submit("add_events", gid, 1, [ev(0, "GAME_START"), ev(1)])
    store.submit("add_snapshots", gid, 1, [snap(1)])
    store.submit("add_decisions", gid, 1, [dec(1)])

    assert store.submit("delete_generation", gid, 0) == 4

    conn = store.read()
    try:
        gens = [r[0] for r in conn.execute(
            "SELECT parse_generation FROM events WHERE game_id=?", (gid,))]
        nsnap = conn.execute(
            "SELECT COUNT(*) FROM snapshots WHERE game_id=?",
            (gid,)).fetchone()[0]
        ndec = conn.execute(
            "SELECT COUNT(*) FROM decisions WHERE game_id=?",
            (gid,)).fetchone()[0]
    finally:
        conn.close()
    assert gens == [1, 1]
    assert (nsnap, ndec) == (1, 1)
    assert len(store.get_events(gid)) == 2
    assert store.max_generation(gid) == 1


# -- reviews / library --------------------------------------------------

def test_pending_review_inserted_before_work_is_resumable(store):
    gid = mkgame(store)
    store.submit("upsert_review", gid, "pending")
    assert store.pending_reviews() == [gid]
    assert store.get_review(gid)["status"] == "pending"

    store.submit("upsert_review", gid, "ready",
                 {"summary": {"headline": "ok"},
                  "evaluator_version": "eval-0.1.0+hs2"})
    assert store.pending_reviews() == []
    row = store.get_review(gid)
    assert row["status"] == "ready"
    assert json.loads(row["summary"])["headline"] == "ok"
    assert row["evaluator_version"] == "eval-0.1.0+hs2"


def test_list_games_filters(store):
    deck_id = store.submit("upsert_deck", {
        "deckstring": "AAECAQ==", "name": "Burn Mage", "class": "MAGE",
        "cards": [["Fireball", 2]], "source": "user"})
    assert store.submit("upsert_deck", {
        "deckstring": "AAECAQ==", "name": "Burn Mage v2",
        "cards": [["Fireball", 2]], "source": "user"}) == deck_id

    mkgame(store, started_at=1.0, player_class="MAGE", result="win",
           deck_id=deck_id)
    mkgame(store, started_at=2.0, player_class="MAGE", result="loss")
    mkgame(store, started_at=3.0, player_class="ROGUE", result="win")

    assert len(store.list_games()) == 3
    assert [g["started_at"] for g in store.list_games()] == [3.0, 2.0, 1.0]
    assert len(store.list_games(cls="MAGE")) == 2
    assert len(store.list_games(result="win")) == 2
    assert len(store.list_games(cls="MAGE", result="win")) == 1
    assert len(store.list_games(deck_id=deck_id)) == 1
    assert len(store.list_games(limit=1)) == 1


def test_meta_deck_and_source_upserts(store):
    mid = store.submit("upsert_meta_deck", {
        "name": "Herald Rogue", "class": "ROGUE", "archetype": "tempo",
        "cards": [["Preparation", 2]], "source": "user_paste"})
    assert store.submit("upsert_meta_deck", {
        "name": "Herald Rogue", "class": "ROGUE",
        "cards": [["Preparation", 1]], "source": "vs_report"}) == mid
    sid = store.submit("add_source", {"kind": "hsjson", "ok": 1})
    conn = store.read()
    try:
        row = conn.execute("SELECT * FROM meta_decks WHERE id=?",
                           (mid,)).fetchone()
        assert conn.execute("SELECT kind FROM sources WHERE id=?",
                            (sid,)).fetchone()["kind"] == "hsjson"
    finally:
        conn.close()
    assert row["source"] == "vs_report"
    assert json.loads(row["cards"]) == [["Preparation", 1]]


# -- jsonl migration ----------------------------------------------------

def write_jsonl(path, records):
    with open(path, "w", encoding="utf-8") as f:
        for rec in records:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        f.write("\n")            # trailing blank line, as seen in the wild


def test_migrate_handles_both_legacy_schemas(store, tmp_path):
    path = str(tmp_path / "games.jsonl")
    write_jsonl(path, [LIVE_LINE, WATCHER_LINE])

    assert migrate(store, path) == {"imported": 2, "skipped": 0}

    games = store.list_games()
    assert len(games) == 2
    by_ts = {round(g["started_at"], 2): g for g in games}
    live = by_ts[round(LIVE_LINE["ts"], 2)]
    watch = by_ts[round(WATCHER_LINE["ts"], 2)]

    assert live["result"] == "win"
    assert live["player_class"] == "MAGE"
    assert live["opponent_class"] == "ROGUE"
    assert live["player_name"] == "Andrij"
    assert live["opponent_name"] == "EnemyRogue"
    assert live["turns"] == 2
    assert live["mode"] == "unknown"

    assert watch["result"] == "loss"
    assert watch["player_name"] == "Andrij"
    assert watch["opponent_name"] == "EnemyPriest"
    assert watch["player_class"] is None

    for g in games:
        assert g["log_hash"]
        assert store.get_events(g["id"]) == []
        assert store.get_review(g["id"])["status"] == "partial"
    assert store.pending_reviews() == []


def test_migrate_is_idempotent(store, tmp_path):
    path = str(tmp_path / "games.jsonl")
    write_jsonl(path, [LIVE_LINE, WATCHER_LINE])
    assert migrate(store, path) == {"imported": 2, "skipped": 0}
    assert migrate(store, path) == {"imported": 0, "skipped": 2}
    assert len(store.list_games()) == 2

    # appending new games only imports the new ones
    with open(path, "a", encoding="utf-8") as f:
        f.write(json.dumps({**LIVE_LINE, "ts": 99.0}) + "\n")
    assert migrate(store, path) == {"imported": 1, "skipped": 2}
    assert len(store.list_games()) == 3


def test_migrate_skips_unparsable_lines_and_missing_file(store, tmp_path):
    path = str(tmp_path / "games.jsonl")
    with open(path, "w", encoding="utf-8") as f:
        f.write("not json\n")
        f.write("[1, 2]\n")
        f.write(json.dumps(WATCHER_LINE) + "\n")
    assert migrate(store, path) == {"imported": 1, "skipped": 2}
    assert migrate(store, str(tmp_path / "nope.jsonl")) == {
        "imported": 0, "skipped": 0}


def test_migrate_cli_module(tmp_path):
    import subprocess
    import sys as _sys
    path = tmp_path / "games.jsonl"
    write_jsonl(str(path), [LIVE_LINE])
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = subprocess.run(
        [_sys.executable, "-m", "store.migrate_jsonl",
         "--jsonl", str(path), "--workdir", str(tmp_path)],
        cwd=root, capture_output=True, text=True, timeout=120)
    assert out.returncode == 0, out.stderr
    assert "imported 1" in out.stdout
    assert (tmp_path / "tavernlab.sqlite").exists()
