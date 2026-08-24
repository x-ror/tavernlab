"""Local SQLite store: one writer thread, many short-lived readers.

SQLite in WAL mode serves any number of concurrent readers but exactly
one writer.  Instead of scattering retry loops over every HTTP handler,
job thread and tailer, all writes are funnelled through a single thread
that owns the only write connection (design §2.4).  Callers hand it an
op name plus arguments and block on a `Future`, so a write still reads
like a plain function call but can never collide with another one and
never raises "database is locked".

Readers get their own connection per call: a `sqlite3.Connection` must
not be shared across threads, and in WAL a reader never blocks the
writer, so a fresh connection is both the safe and the cheap option.
"""
import contextlib
import json
import os
import queue
import sqlite3
import threading
import time
from concurrent.futures import Future

DB_NAME = "tavernlab.sqlite"
SCHEMA_VERSION = 1
SCHEMA_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           "schema.sql")

# Child rows of one game can come from more than one parse: the v1 live
# tail writes events at generation 0 while the game is still running and
# finalize re-imports the finished game from hslog at generation 1.  A
# read must therefore see exactly one parse.  MAX() is taken over
# `events` and never over the child table itself, because events is the
# only table the tail writes -- so it is the only table whose MAX is the
# true current generation even before snapshots/decisions exist.  The
# `> 0` then drops an in-progress tail entirely rather than mixing gen-0
# regex tags with gen-1 packets (design §2.2, Generation rule).
GEN_WHERE = ("parse_generation = (SELECT MAX(parse_generation) FROM "
             "events e2 WHERE e2.game_id = ?) AND parse_generation > 0")

_STOP = object()

# Columns whose value may be handed in as a dict/list and is stored as
# JSON text.
_JSON_COLS = frozenset((
    "payload", "visible", "unimplemented", "chosen", "alternatives",
    "lethal_plan", "explanation", "summary", "key_moments", "cards",
    "provenance",
))

# NOT NULL columns that carry a DDL default: passing None would trip the
# constraint, so fill the default here instead.
_ZERO_COLS = frozenset((
    "lethal_ok", "search_ok", "actions_complete", "search_depth",
))

_EVENT_COLS = ("seq", "ts_log", "type", "payload")
_SNAPSHOT_COLS = ("event_seq", "visible", "lethal_ok", "search_ok",
                  "unimplemented", "wp", "wp_source")
_DECISION_COLS = ("event_seq", "turn", "side", "kind", "chosen",
                  "alternatives", "actions_complete", "lethal_ok",
                  "search_ok", "wp_before", "wp_after", "delta_wp",
                  "label", "label_conf", "lethal_available",
                  "lethal_plan", "explanation", "search_depth",
                  "evaluator_version")


def _statements(sql):
    """Split a script into statements using sqlite's own completeness
    test, so `--` comments and quoted text cannot split a statement."""
    buf = ""
    for line in sql.splitlines(keepends=True):
        buf += line
        if sqlite3.complete_statement(buf):
            yield buf.strip()
            buf = ""
    tail = buf.strip()
    if tail:
        yield tail


def _is_pragma(stmt):
    for line in stmt.splitlines():
        line = line.strip()
        if not line or line.startswith("--"):
            continue
        return line.upper().startswith("PRAGMA")
    return False


def _enc(col, value):
    if col in _ZERO_COLS and value is None:
        return 0
    if col in _JSON_COLS and value is not None \
            and not isinstance(value, (str, bytes)):
        return json.dumps(value, ensure_ascii=False)
    return value


@contextlib.contextmanager
def _tx(conn):
    """Explicit transaction: the writer connection is in autocommit
    (`isolation_level=None`), so multi-statement ops bracket themselves."""
    conn.execute("BEGIN")
    try:
        yield
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    conn.execute("COMMIT")


class Store:
    """Owns `tavernlab.sqlite`: `submit()` writes, `get_*()` reads.

    `path` is the database file; a directory is accepted and gets
    `tavernlab.sqlite` appended, so both `Store(workdir)` and
    `Store(workdir + "/tavernlab.sqlite")` do the right thing.
    """

    def __init__(self, path):
        path = os.path.abspath(path)
        if os.path.isdir(path) or not path.endswith(".sqlite"):
            path = os.path.join(path, DB_NAME)
        parent = os.path.dirname(path)
        if parent and not os.path.isdir(parent):
            os.makedirs(parent, exist_ok=True)
        self.path = path
        self._cols = {}
        self._closed = False
        self._lock = threading.Lock()
        self._q = queue.Queue()
        self._ensure_schema()
        self._thread = threading.Thread(target=self._loop,
                                        name="sqlite-writer", daemon=True)
        self._thread.start()

    # -- setup ----------------------------------------------------------

    def _ensure_schema(self):
        """Create the schema on first open.  Runs on the constructing
        thread with its own connection, which is closed again before the
        writer thread starts, so no connection is ever shared."""
        conn = sqlite3.connect(self.path, isolation_level=None)
        try:
            conn.execute("PRAGMA busy_timeout=5000")
            with open(SCHEMA_PATH, encoding="utf-8") as f:
                script = f.read()
            stmts = list(_statements(script))
            for stmt in stmts:
                if _is_pragma(stmt):
                    conn.execute(stmt)      # WAL cannot be set in a tx
            # BEGIN IMMEDIATE so two processes opening the same fresh db
            # cannot both decide the schema is missing.
            conn.execute("BEGIN IMMEDIATE")
            try:
                if self._needs_schema(conn):
                    for stmt in stmts:
                        if not _is_pragma(stmt):
                            conn.execute(stmt)
                    conn.execute(
                        "INSERT INTO schema_migrations (version, applied_at)"
                        " VALUES (?, ?)", (SCHEMA_VERSION, time.time()))
            except BaseException:
                conn.execute("ROLLBACK")
                raise
            conn.execute("COMMIT")
            for table in ("games", "events", "snapshots", "decisions",
                          "reviews", "decks", "cards", "meta_decks",
                          "sources", "settings"):
                self._cols[table] = frozenset(
                    r[1] for r in conn.execute(
                        'PRAGMA table_info("%s")' % table))
        finally:
            conn.close()

    @staticmethod
    def _needs_schema(conn):
        row = conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table'"
            " AND name='schema_migrations'").fetchone()
        if not row:
            return True
        n = conn.execute("SELECT COUNT(*) FROM schema_migrations").fetchone()
        return not n[0]

    # -- writer ---------------------------------------------------------

    def _loop(self):
        conn = sqlite3.connect(self.path, isolation_level=None)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA busy_timeout=5000")
        conn.execute("PRAGMA foreign_keys=ON")
        try:
            while True:
                item = self._q.get()
                if item is _STOP:
                    return
                fn_name, args, fut = item
                if not fut.set_running_or_notify_cancel():
                    continue
                try:
                    fn = getattr(self, "_w_" + fn_name)
                except AttributeError:
                    fut.set_exception(
                        AttributeError("no writer op %r" % (fn_name,)))
                    continue
                try:
                    fut.set_result(fn(conn, *args))
                except BaseException as e:      # noqa: BLE001 - relayed
                    if conn.in_transaction:
                        conn.execute("ROLLBACK")
                    fut.set_exception(e)
        finally:
            conn.close()

    def submit(self, fn_name, *args):
        """Run writer op `_w_<fn_name>` on the writer thread and return
        its result.  Exceptions are re-raised in the calling thread."""
        fut = Future()
        with self._lock:
            if self._closed:
                raise RuntimeError("store is closed")
            self._q.put((fn_name, args, fut))
        return fut.result(timeout=30)

    def close(self):
        """Drain the queue and stop the writer thread.  Ops already
        queued still run: the sentinel is FIFO behind them."""
        with self._lock:
            if self._closed:
                return
            self._closed = True
            self._q.put(_STOP)
        self._thread.join(timeout=30)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    # -- writer ops -----------------------------------------------------

    def _check(self, table, keys):
        cols = self._cols[table]
        bad = [k for k in keys if k not in cols]
        if bad:
            raise ValueError("unknown %s columns: %s"
                             % (table, ", ".join(sorted(bad))))
        return list(keys)

    def _insert(self, conn, table, data):
        keys = self._check(table, data)
        sql = 'INSERT INTO "%s" (%s) VALUES (%s)' % (
            table, ", ".join('"%s"' % k for k in keys),
            ", ".join("?" * len(keys)))
        cur = conn.execute(sql, [_enc(k, data[k]) for k in keys])
        return cur.lastrowid

    def _bulk(self, conn, table, cols, game_id, generation, rows,
              auto_seq=False):
        sql = 'INSERT INTO "%s" (game_id, parse_generation, %s)' \
              ' VALUES (%s)' % (
                  table, ", ".join('"%s"' % c for c in cols),
                  ", ".join("?" * (len(cols) + 2)))
        data = []
        for i, row in enumerate(rows):
            vals = [game_id, generation]
            for col in cols:
                v = row.get(col)
                if v is None and auto_seq and col == cols[0]:
                    v = i
                vals.append(_enc(col, v))
            data.append(vals)
        if not data:
            return 0
        with _tx(conn):
            conn.executemany(sql, data)
        return len(data)

    def _w_create_game(self, conn, game):
        row = dict(game)
        now = time.time()
        row.setdefault("started_at", now)
        row.setdefault("created_at", now)
        row.setdefault("mode", "unknown")
        return self._insert(conn, "games", row)

    def _w_update_game(self, conn, game_id, fields):
        keys = self._check("games", fields)
        if not keys:
            return 0
        sql = 'UPDATE games SET %s WHERE id = ?' % ", ".join(
            '"%s" = ?' % k for k in keys)
        cur = conn.execute(sql, [_enc(k, fields[k]) for k in keys]
                           + [game_id])
        return cur.rowcount

    def _w_set_raw_power(self, conn, game_id, blob):
        cur = conn.execute("UPDATE games SET raw_power = ? WHERE id = ?",
                           (memoryview(blob) if blob is not None else None,
                            game_id))
        return cur.rowcount

    def _w_add_events(self, conn, game_id, generation, rows):
        return self._bulk(conn, "events", _EVENT_COLS, game_id, generation,
                          rows, auto_seq=True)

    def _w_add_snapshots(self, conn, game_id, generation, rows):
        return self._bulk(conn, "snapshots", _SNAPSHOT_COLS, game_id,
                          generation, rows)

    def _w_replace_snapshots(self, conn, game_id, generation, rows):
        """Same reason as `_w_replace_decisions`: re-reviewing one parse
        is routine, and `UNIQUE(game_id, parse_generation, event_seq)`
        would abort the second run."""
        with _tx(conn):
            conn.execute("DELETE FROM snapshots WHERE game_id = ?"
                         " AND parse_generation = ?",
                         (game_id, generation))
        return self._bulk(conn, "snapshots", _SNAPSHOT_COLS, game_id,
                          generation, rows)

    def _w_add_decisions(self, conn, game_id, generation, rows):
        return self._bulk(conn, "decisions", _DECISION_COLS, game_id,
                          generation, rows)

    def _w_replace_decisions(self, conn, game_id, generation, rows):
        """Re-review the same parse: drop this generation's decisions
        first.  `UNIQUE(game_id, parse_generation, event_seq, kind)`
        means a second run of the evaluator would otherwise abort, and
        re-running it is the normal path after an evaluator change."""
        with _tx(conn):
            conn.execute("DELETE FROM decisions WHERE game_id = ?"
                         " AND parse_generation = ?",
                         (game_id, generation))
        return self._bulk(conn, "decisions", _DECISION_COLS, game_id,
                          generation, rows)

    def _w_upsert_review(self, conn, game_id, status, fields=None, **extra):
        # `submit()` carries positional args only (§2.4), so callers going
        # through the queue pass the optional columns as one dict.
        data = dict(fields or {})
        data.update(extra)
        data["game_id"] = game_id
        data["status"] = status
        data.setdefault("created_at", time.time())
        keys = self._check("reviews", data)
        upd = [k for k in keys if k not in ("game_id", "created_at")]
        sql = ('INSERT INTO reviews (%s) VALUES (%s)'
               ' ON CONFLICT(game_id) DO UPDATE SET %s' % (
                   ", ".join('"%s"' % k for k in keys),
                   ", ".join("?" * len(keys)),
                   ", ".join('"%s" = excluded."%s"' % (k, k) for k in upd)))
        conn.execute(sql, [_enc(k, data[k]) for k in keys])
        return game_id

    def _w_delete_generation(self, conn, game_id, generation):
        """Drop one parse of a game whole.  The v1 tail finalize path
        deletes generation 0 after the hslog reparse landed as 1."""
        n = 0
        with _tx(conn):
            for table in ("decisions", "snapshots", "events"):
                cur = conn.execute(
                    'DELETE FROM "%s" WHERE game_id = ?'
                    ' AND parse_generation = ?' % table,
                    (game_id, generation))
                n += cur.rowcount
        return n

    def _w_set_setting(self, conn, key, value):
        if not isinstance(value, str):
            value = json.dumps(value, ensure_ascii=False)
        conn.execute("INSERT INTO settings (key, value) VALUES (?, ?)"
                     " ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                     (key, value))
        return key

    def _w_upsert_deck(self, conn, deck):
        row = dict(deck)
        row.setdefault("created_at", time.time())
        row.setdefault("source", "user")
        return self._upsert_named(conn, "decks", row, "deckstring")

    def _w_upsert_meta_deck(self, conn, deck):
        return self._upsert_named(conn, "meta_decks", dict(deck), "name")

    def _upsert_named(self, conn, table, row, key):
        """Insert or update on the table's UNIQUE text key, returning the
        row id.  RETURNING is avoided: it needs sqlite >= 3.35 and the
        frozen builds may carry an older library."""
        keys = self._check(table, row)
        if row.get(key) is None:
            return self._insert(conn, table, row)
        upd = [k for k in keys if k != key]
        sql = 'INSERT INTO "%s" (%s) VALUES (%s)' % (
            table, ", ".join('"%s"' % k for k in keys),
            ", ".join("?" * len(keys)))
        if upd:
            sql += ' ON CONFLICT("%s") DO UPDATE SET %s' % (
                key, ", ".join('"%s" = excluded."%s"' % (k, k) for k in upd))
        else:
            sql += ' ON CONFLICT("%s") DO NOTHING' % key
        conn.execute(sql, [_enc(k, row[k]) for k in keys])
        got = conn.execute('SELECT id FROM "%s" WHERE "%s" = ?'
                           % (table, key), (row[key],)).fetchone()
        return got[0] if got else None

    def _w_add_source(self, conn, source):
        row = dict(source)
        row.setdefault("fetched_at", time.time())
        return self._insert(conn, "sources", row)

    # -- readers --------------------------------------------------------

    def read(self):
        """A fresh read connection.  Never cache or share one across
        threads: `sqlite3.Connection` is not thread-safe, and WAL makes
        an extra reader nearly free.  The caller closes it."""
        conn = sqlite3.connect(self.path)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA busy_timeout=5000")
        return conn

    def _query(self, sql, params=(), one=False):
        conn = self.read()
        try:
            cur = conn.execute(sql, params)
            return cur.fetchone() if one else cur.fetchall()
        finally:
            conn.close()

    def _query_gen(self, table, game_id, extra="", params=(), order=""):
        """Read child rows of the single newest finished parse.  All
        generation-aware reads go through here so GEN_WHERE exists once.
        Parameter order matters: the outer `game_id` binds first, then
        the one inside the MAX() subquery, then `extra`'s own params."""
        sql = 'SELECT * FROM "%s" WHERE game_id = ? AND %s' % (table,
                                                               GEN_WHERE)
        if extra:
            sql += " " + extra
        if order:
            sql += " " + order
        return self._query(sql, (game_id, game_id) + tuple(params))

    def list_games(self, limit=50, cls=None, result=None, deck_id=None):
        sql = "SELECT * FROM games WHERE 1=1"
        params = []
        if cls:
            sql += " AND player_class = ?"
            params.append(cls)
        if result:
            sql += " AND result = ?"
            params.append(result)
        if deck_id is not None:
            sql += " AND deck_id = ?"
            params.append(deck_id)
        sql += " ORDER BY started_at DESC, id DESC LIMIT ?"
        params.append(int(limit))
        return self._query(sql, tuple(params))

    def get_game(self, game_id):
        return self._query("SELECT * FROM games WHERE id = ?", (game_id,),
                           one=True)

    def game_id_for_hash(self, log_hash):
        """Import idempotency: `log_hash` is the sha1 of the Power.log
        slice, so re-importing a session inserts nothing."""
        row = self._query("SELECT id FROM games WHERE log_hash = ?",
                          (log_hash,), one=True)
        return None if row is None else row["id"]

    def get_events(self, game_id, from_seq=0):
        return self._query_gen("events", game_id, "AND seq >= ?",
                               (from_seq,), "ORDER BY seq")

    def get_snapshots(self, game_id):
        return self._query_gen("snapshots", game_id,
                               order="ORDER BY event_seq")

    def get_decisions(self, game_id):
        return self._query_gen("decisions", game_id,
                               order="ORDER BY event_seq, id")

    def get_review(self, game_id):
        return self._query("SELECT * FROM reviews WHERE game_id = ?",
                           (game_id,), one=True)

    def pending_reviews(self):
        """Game ids to re-queue at startup; `JOBS` is progress-only."""
        rows = self._query("SELECT game_id FROM reviews"
                           " WHERE status='pending'")
        return [r["game_id"] for r in rows]

    def max_generation(self, game_id):
        """Newest parse generation, or None when the game has no events
        (0 is a real generation: an in-progress live tail)."""
        row = self._query("SELECT MAX(parse_generation) AS g FROM events"
                          " WHERE game_id = ?", (game_id,), one=True)
        return None if row is None or row["g"] is None else row["g"]

    def get_setting(self, key, default=None):
        row = self._query("SELECT value FROM settings WHERE key = ?",
                          (key,), one=True)
        return default if row is None else row["value"]

    def all_settings(self):
        return {r["key"]: r["value"]
                for r in self._query("SELECT key, value FROM settings")}


def open_store(workdir):
    """Open `{workdir}/tavernlab.sqlite` (design §2.9)."""
    return Store(os.path.join(workdir, DB_NAME))
