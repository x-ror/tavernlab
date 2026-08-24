PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at REAL NOT NULL
);

CREATE TABLE games (
  id INTEGER PRIMARY KEY,
  started_at REAL NOT NULL,
  ended_at REAL,
  mode TEXT NOT NULL DEFAULT 'unknown',     -- ranked_standard, ranked_wild, casual, friendly, arena, bg, unknown
  format TEXT,                              -- standard, wild, twist
  player_name TEXT,
  player_id INTEGER,                        -- 1 or 2 in log
  player_class TEXT,
  opponent_name TEXT,
  opponent_class TEXT,
  opponent_archetype TEXT,
  opponent_archetype_conf REAL,
  deck_id INTEGER REFERENCES decks(id),
  deckstring TEXT,
  result TEXT,                              -- win, loss, tie, unknown
  turns INTEGER,
  going_first INTEGER,                      -- 0/1
  log_dir TEXT,
  log_hash TEXT,                            -- sha1 of Power.log slice
  raw_power BLOB,                           -- gzip of last-game Power.log slice (required on import)
  notes TEXT,
  created_at REAL NOT NULL
);
CREATE INDEX idx_games_started ON games(started_at);
CREATE INDEX idx_games_deck ON games(deck_id);
CREATE INDEX idx_games_result ON games(result, opponent_class);

CREATE TABLE events (
  id INTEGER PRIMARY KEY,
  game_id INTEGER NOT NULL REFERENCES games(id),
  parse_generation INTEGER NOT NULL DEFAULT 1,
  seq INTEGER NOT NULL,
  ts_log TEXT,
  type TEXT NOT NULL,
  payload TEXT NOT NULL,                    -- JSON
  UNIQUE(game_id, parse_generation, seq)
);
CREATE INDEX idx_events_game ON events(game_id, parse_generation, seq);

CREATE TABLE snapshots (
  id INTEGER PRIMARY KEY,
  game_id INTEGER NOT NULL REFERENCES games(id),
  parse_generation INTEGER NOT NULL,
  event_seq INTEGER NOT NULL,
  visible TEXT NOT NULL,                    -- JSON VisibleState
  lethal_ok INTEGER NOT NULL DEFAULT 0,     -- stats overlay safe for find_lethal
  search_ok INTEGER NOT NULL DEFAULT 0,     -- trigger graph complete; MVP always 0
  unimplemented TEXT,                       -- JSON string[]
  wp REAL,
  wp_source TEXT,                           -- logistic_v1 (always hatch), gbdt_v1, none
  UNIQUE(game_id, parse_generation, event_seq)
);

CREATE TABLE decisions (
  id INTEGER PRIMARY KEY,
  game_id INTEGER NOT NULL REFERENCES games(id),
  parse_generation INTEGER NOT NULL,
  event_seq INTEGER NOT NULL,
  turn INTEGER,
  side TEXT NOT NULL,                       -- us, them
  kind TEXT NOT NULL,                       -- mulligan, play, attack, hero_power, location, prepare, discover, choose, end_turn
  chosen TEXT NOT NULL,                     -- JSON Action
  alternatives TEXT,                        -- JSON Action[] | null
  actions_complete INTEGER NOT NULL DEFAULT 0,  -- 0 => no skill glyph
  lethal_ok INTEGER NOT NULL DEFAULT 0,
  search_ok INTEGER NOT NULL DEFAULT 0,
  wp_before REAL,
  wp_after REAL,
  delta_wp REAL,                            -- MVP: stored if computed, NEVER used to rank or label
  label TEXT,                               -- see §3.3; NULL if hidden
  label_conf REAL,
  lethal_available INTEGER,
  lethal_plan TEXT,
  explanation TEXT NOT NULL,                -- JSON Explanation
  search_depth INTEGER NOT NULL DEFAULT 0,
  evaluator_version TEXT NOT NULL,
  UNIQUE(game_id, parse_generation, event_seq, kind)
);

CREATE TABLE reviews (
  game_id INTEGER PRIMARY KEY REFERENCES games(id),
  status TEXT NOT NULL,                     -- pending | ready | partial | error
  -- INSERT pending BEFORE work starts so restart can resume.
  summary TEXT,                             -- JSON Report
  key_moments TEXT,                         -- JSON
  evaluator_version TEXT,
  created_at REAL NOT NULL,
  error TEXT
);

CREATE TABLE decks (
  id INTEGER PRIMARY KEY,
  deckstring TEXT UNIQUE,
  name TEXT,
  class TEXT,
  format TEXT,
  cards TEXT NOT NULL,                      -- JSON [[name, n], ...]
  source TEXT NOT NULL,                     -- user, meta, imported
  created_at REAL NOT NULL
);

CREATE TABLE cards (
  card_id TEXT PRIMARY KEY,
  dbf_id INTEGER,
  name TEXT NOT NULL,
  set_id TEXT,
  class TEXT,
  type TEXT,
  cost INTEGER,
  collectible INTEGER NOT NULL,
  implemented INTEGER NOT NULL,
  notes TEXT,
  text TEXT,
  hsjson_build TEXT
);
CREATE INDEX idx_cards_dbf ON cards(dbf_id);
CREATE INDEX idx_cards_name ON cards(name);

CREATE TABLE meta_decks (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  class TEXT NOT NULL,
  archetype TEXT,
  deckstring TEXT,
  cards TEXT NOT NULL,
  source TEXT NOT NULL,                     -- user_paste, file, hsjson, vs_report
  source_url TEXT,
  fetched_at REAL,
  provenance TEXT                           -- JSON
);

CREATE TABLE sources (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,                       -- hsjson, blizzard_api, user, vs
  url TEXT,
  fetched_at REAL,
  etag TEXT,
  bytes INTEGER,
  license_note TEXT,
  ok INTEGER
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
