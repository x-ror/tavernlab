# TavernLab (hearthstone-sim)

Local Hearthstone constructed-deck lab: a zero-dependency Rust rules engine
(`tavernlab-sim/`) plus a React UI (`web/`), served by one binary
(`tavernsim serve`). Everything runs offline; the app makes no network calls.
User-facing docs (README.md, docs/, Makefile help) are in Ukrainian; code and
doc-comments are in English.

## Commands

- Build: `cargo build --manifest-path tavernlab-sim/Cargo.toml` (or `make build`; `PROFILE=release` for optimized)
- Test: `cargo test --manifest-path tavernlab-sim/Cargo.toml` — there is no `make test`
- Run the app: `make serve` → http://127.0.0.1:8765. Fresh clone needs `make web` once (`web/dist` is a build artifact, not checked in)
- Regenerate the card table after editing `data/*_cards.json`: `cargo run -p xtask --manifest-path tavernlab-sim/Cargo.toml -- cards`
- Measurements (slow, deliberate, NOT part of tests): `make bench|policy|weights|mulligan|tiers|callgrind`

## Hard rules (from docs/DESIGN.md and crate docs — do not break)

1. **Zero third-party dependencies**, Rust and runtime. No serde, no HTTP
   framework, no rusqlite — `json/` and `sqlite/` exist because of this.
   Adding a crate is a design change requiring explicit user approval.
2. **`game::Game` is a fixed-size value that clones via memcpy.** No
   `Vec`/`HashMap`/`String`/`Rc` in live game state; boards/hands/decks are
   inline arrays, cards are 16-bit `CardId` indices into one immutable table.
   Any allocation in the simulation hot path is a design bug.
3. **No user-facing prose in Rust.** All strings live in `locales/en.json`
   and `locales/uk.json` (both must be updated together; `cli/tests/strings.rs`
   checks the pair). The watcher emits structured `Line { key, args }`,
   rendered by the front end.
4. **The log watcher never guesses.** `cli/src/watch/tracker.rs` records only
   what `Power.log` states outright; unknowns stay `None`. It reads official
   log files only — no memory reads. Since 2026-08-31 read-only memory
   access to the user's own running client is allowed (`memreader/`,
   docs/DESIGN.md "Правовий режим") but stays a **separate** source: never
   merged into `tracker.rs`'s log-only reconstruction, never falls back
   silently between the two. No traffic interception, no input emulation,
   ever — that boundary hasn't moved.
5. **Doctests are disabled workspace-wide** (`doctest = false`) — do not add
   runnable `///` examples.
6. **Data separation.** The repo holds shipped data (corpora, gauntlets,
   locales); per-user state lives in `~/.local/share/TavernLab`
   (`TAVERNLAB_HOME` overrides). Never write user state into the checkout.
7. **Never run `cargo fmt` across the workspace.** The tree is deliberately
   not rustfmt-clean (~376 diffs, 291 of them the compact one-row card
   entries in `behaviour.rs`). Match the surrounding style by hand instead.

## Architecture map

- `tavernlab-sim/core/` — the rules kernel.
  - `cards/table.rs` — **100k GENERATED lines** (`xtask -- cards` from
    `data/*_cards.json`). Never edit by hand; never grep it casually.
  - `cards/behaviour.rs` — hand-written per-card effects, keyed **by card
    name** (so reprints share behaviour). Each effect is a non-capturing
    closure coerced to a `fn` pointer. Adding a card should be one row here;
    if it can't be, add a reusable verb to `effects.rs` instead.
  - `game.rs` / `state.rs` / `effects.rs` / `events.rs` — turn loop, inline
    state, effect verbs, triggers. `agent.rs` (greedy policy),
    `planner.rs` (within-turn search), `batch.rs` (multithreaded runs —
    must stay deterministic across thread counts), `optimize.rs`,
    `gauntlet.rs`, `tiers.rs`, `deckstring.rs`.
- `tavernlab-sim/cli/` — the `tavernsim` binary.
  - `serve/` — hand-rolled HTTP server (`api.rs` is the `/api` surface,
    `jobs.rs` async job queue, `live.rs` hosts the watcher in the server).
  - `watch/` — live `Power.log` reader: `log.rs` (scanner, no regex),
    `tracker.rs` (game reconstruction), `advice.rs`, `mod.rs`.
    Log dir: `HS_LOGS` env or `--logs` flag on Linux/Wine.
- `tavernlab-sim/memreader/` — read-only Mono memory reader for the user's
  own running Hearthstone client (`process_vm_readv`, no third-party crate).
  `--snapshot` prints one JSON document to stdout (all diagnostics go to
  stderr); confirmed-live offsets are in `src/mono_layout.rs`. See its
  README before touching it — it explains what's confirmed vs. guessed.
- `tavernlab-sim/json/`, `sqlite/` — dependency-free format readers/writers.
- `tavernlab-sim/xtask/` — codegen: `cards`, `wild-gauntlet`, `backfill`, `runes`.
- `web/` — React 18 + React Spectrum + Vite. Strings via `i18n.jsx`/locales.
- `data/` — committed xtask inputs: card corpora, gauntlet decklists,
  `runes.json`. The Python pipeline that built the corpora is gone; the next
  Standard rotation needs it rewritten (see README «Що пішло разом із Python»).

## Tests

- Unit tests in-file; big integration suites: `core/tests/cards.rs` (13.7k
  lines — one conformance test per implemented card: fixed board, play the
  card, assert state), `core/tests/{runes,graveyard,gauntlet}.rs`,
  `cli/tests/{watch,serve,strings,real_decks}.rs` (serve/watch tests boot the
  real binary), `sqlite/tests/format.rs`.
- Every implemented card gets a conformance test written from the card's
  actual text/rulings, not from the implementation.

## Measurement discipline

- The README is the claims file: every number in it must be reproducible by a
  `make` target on this checkout. When a change moves a number, re-measure and
  update the README — never leave a stale record.
- Never compare timings naively. Use `tools/ab-bench.sh` (read its header) and
  run the A/A control first; interleaved A,B,B,A ordering. `make callgrind`
  gives deterministic instruction counts when seconds are too noisy.
- Control columns in `policy`/`weights`/`mulligan` must read 50.0% — anything
  else means the harness is broken, not the change.
- `bench` throughput legitimately drops as more cards get real
  implementations; report what this build measures.

## Search hygiene

Exclude from repo-wide searches: `tavernlab-sim/target/` (~2 GB),
`web/node_modules` (~170 MB), `tavernlab-sim/core/src/cards/table.rs`, and
`core/tests/cards.rs` unless card tests are the subject. README.md is 90 KB —
grep for the section header you need, don't read it whole.
