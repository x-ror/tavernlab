---
name: add-card
description: Implement a Hearthstone card's effect in the TavernLab simulator — behaviour row, conformance test, README coverage. Use whenever the user asks to add, implement, or fix a card's effect.
---

# Implementing a card

## Order of work

1. **Source the card's real behaviour first.** Read the card's actual text in
   `data/standard_cards.json` / `data/wild_cards.json` (grep by name). If the
   effect has rulings or edge cases (Deathrattle ordering, aura stacking,
   corpse spending), write down what the card *actually* does before writing
   code — see how commits like "Husk checked against the card's own rulings"
   were done. If the card is missing from the corpus, `cargo run -p xtask
   --manifest-path tavernlab-sim/Cargo.toml -- backfill` may fill it; then
   `-- cards` to regenerate the table.

2. **One row in `BEHAVIOURS`.** Add the card to
   `tavernlab-sim/core/src/cards/behaviour.rs`, keyed by card **name** (not
   id — reprints share the row). Effects are non-capturing closures coerced
   to `fn` pointers. Compose from the existing verbs in
   `core/src/effects.rs`; if the card needs a mechanic no verb expresses,
   add the verb to `effects.rs` first, generically, then use it. A card that
   needs a page of bespoke code in behaviour.rs is a smell.

3. **Never edit `core/src/cards/table.rs`** — it is generated. Stats, cost,
   keywords, races come from the corpus via xtask.

4. **Conformance test in `core/tests/cards.rs`.** One test per card, in the
   established pattern: fixed board, play the card, assert the resulting
   state. Write the assertions from the card text/rulings gathered in step 1,
   not from what the implementation happens to do. Cover the edge case that
   made the card worth implementing.

5. **Run the suite:** `cargo test --manifest-path tavernlab-sim/Cargo.toml`.

6. **Keep the README honest.** The README carries a card-coverage report and
   measured numbers. If coverage counts or claims change, update them
   (README prose is Ukrainian). `bench` throughput legitimately drops as more
   cards get real effects — if a number is quoted, re-measure on this build
   rather than keeping the old record.

## Constraints that bite here

- No allocation in live game state: effects mutate the inline fixed-size
  `state.rs` structures; no `Vec`/`String`/`Box` in anything a `Game` holds.
- `batch.rs` runs must stay deterministic across thread counts — draw RNG
  only through the game's `rng.rs` paths.
- No user-facing strings in Rust; a card implementation should not need any.
