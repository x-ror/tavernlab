//! TavernLab rules kernel.
//!
//! A Hearthstone engine written for one workload: playing millions of complete
//! games as fast as the machine allows, and — later — searching forward from a
//! position. It began beside a Python engine that was the behavioural
//! reference for *what the rules are*, and nothing more; that engine is gone,
//! and this is the whole simulator now.
//!
//! # The one design decision everything follows from
//!
//! [`game::Game`] is a fixed-size value that clones with a `memcpy`. No `Vec`,
//! no `HashMap`, no `String`, no `Rc` anywhere in the live state. Boards, hands
//! and decks are inline arrays; cards are 16-bit indices into one immutable
//! table shared by every thread; keywords are bits in a word.
//!
//! That single constraint is what separates this from a transliteration:
//!
//! * cloning a position costs a memcpy instead of walking an object graph and
//!   remapping entity ids, so lookahead search becomes affordable rather than
//!   theoretical;
//! * a game fits in cache, so a batch run is memory-bandwidth-idle instead of
//!   pointer-chasing;
//! * threads share the card table read-only, so a batch needs no per-worker
//!   copy of the corpus — the Python engine paid ~32 MB per process for Wild;
//! * anything that would need an allocation in the hot path is a design bug,
//!   and shows up as a compile error rather than a slow afternoon in a profiler.
//!
//! Every module below is written to preserve that property. When something
//! cannot be expressed within it, the fix is a better encoding, not a `Box`.
//!
//! # Correctness
//!
//! There is no cross-language differential harness, because there will be no
//! second language to differ from. Correctness is established the way it should
//! have been in the first place: one conformance test per card — a fixed board,
//! the card played, the resulting state asserted — plus rules-level tests for
//! the kernel itself.

pub mod agent;
pub mod batch;
pub mod cards;
pub mod deck;
pub mod deckstring;
pub mod effects;
pub mod gauntlet;
pub mod events;
pub mod game;
pub mod inline;
pub mod optimize;
pub mod rng;
pub mod state;
pub mod telemetry;
pub mod tiers;
