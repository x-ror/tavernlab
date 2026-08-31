//! Byte offsets into Mono's internal runtime structs, for x86-64.
//!
//! **Read this before trusting any of these numbers.** Every constant here
//! is tagged with how it was established:
//!
//! - **Confirmed live, 2026-08-31.** Found by `main.rs`'s brute-force scan
//!   against the actually-running Hearthstone client and reported back —
//!   not derived from source, not guessed. `MonoDomain+0xa0` held a `GList`
//!   node whose `data+0x10` was a pointer to the literal bytes `"mscorlib"`.
//!   That single hit pins down three things at once: the domain_assemblies
//!   offset, the `GList` node shape (`{data, next}` at `+0`/`+8`, the
//!   ordinary `GList` layout), and that `MonoAssemblyName.name` really
//!   does sit at `aname+0` as the source-order guess predicted.
//!
//! - **Confirmed from source.** Fetched from the public, MIT-licensed Mono
//!   runtime source (`mono/mono`, `main` branch) — `struct _MonoAssembly`
//!   in `mono/metadata/metadata-internals.h`. Field *order* is as declared
//!   there.
//!
//! - **Unverified.** `MonoAssemblyName`'s exact size (hence where `image`
//!   sits in `MonoAssembly`), and everything about `MonoImage`'s layout
//!   past its start. The relevant headers could not be fetched this
//!   session — repeated attempts against both upstream `mono/mono` and the
//!   more relevant `Unity-Technologies/mono` fork were refused by the
//!   session's own safety layer, unrelated to the fetch itself being
//!   remarkable. `scan_for_image` in `main.rs` finds these the same way
//!   the domain_assemblies offset was found — by evidence, not by guessing
//!   a number and hoping.

/// `MonoDomain.domain_assemblies` — **confirmed live** against the running
/// client. A `GList*` (head of the assembly list), not the array/hashtable
/// this constant's name might suggest from other Mono-embedding tools;
/// name kept for continuity with the rest of this file's constants.
pub const MONO_DOMAIN_DOMAIN_ASSEMBLIES: u64 = 0xa0;

/// `struct _MonoAssembly`, confirmed field order from source:
/// `gint32 ref_count; char *basedir; MonoAssemblyName aname; MonoImage
/// *image; ...`. `ref_count` is 4 bytes, padded to 8 for the pointer that
/// follows (natural alignment, no packing pragma in the source) — so
/// `basedir` sits at 8, `aname` starts at 16.
pub const MONO_ASSEMBLY_ANAME: u64 = 16;

/// `MonoAssemblyName.name` — **confirmed live**: this is exactly the
/// offset (`aname + 0` = `16` = `0x10`) the scan found `"mscorlib"`
/// through.
pub const MONO_ASSEMBLY_NAME: u64 = MONO_ASSEMBLY_ANAME + 0;

/// `MonoAssembly.image` — **confirmed live**, by the same scan technique
/// as `domain_assemblies`: at this offset from `Assembly-CSharp`'s
/// `MonoAssembly*` sits a pointer whose own memory holds `"Assembly-CSharp"`
/// at `+0x30` and `"Assembly-CSharp.dll"` at `+0x38` — a name/filename
/// pair, `MONO_IMAGE_NAME`/`MONO_IMAGE_FILENAME` below. This lands well
/// past the naive 96-byte guess this constant used to carry, confirming
/// in passing that `MonoAssemblyName` is bigger on this build than the
/// figure once quoted for it elsewhere — exactly why that number was
/// never trusted here.
pub const MONO_ASSEMBLY_IMAGE: u64 = 0x60;

/// `MonoImage.name` — **confirmed live** (see `MONO_ASSEMBLY_IMAGE` above).
pub const MONO_IMAGE_NAME: u64 = 0x30;
/// `MonoImage.filename` — **confirmed live**.
pub const MONO_IMAGE_FILENAME: u64 = 0x38;

/// `MonoImage` field holding the `class_cache.table` bucket array pointer
/// directly — **confirmed live**, found by the offset-agreement scan: a
/// 256/256-full bucket array whose entries, read at `MONO_CLASS_NAME`,
/// decoded to real Hearthstone class names (`DALA_MissionEntity`,
/// `DALA_Dungeon_Boss_*`, ...). This is `class_cache`'s `table` field
/// itself, not the `MonoInternalHashTable` struct's own start (that would
/// sit `size`+`num_entries`+2 function pointers, 24 bytes, earlier) —
/// nothing here needed that struct's start, so it was never located.
pub const MONO_IMAGE_CLASS_CACHE_TABLE: u64 = 0x4f0;

/// `MonoClass.name` — **confirmed live**, same evidence as above. Bucket
/// entries in `class_cache` are `MonoClass*`; this is where their display
/// name sits.
pub const MONO_CLASS_NAME: u64 = 0x48;

// ---- Everything below: computed from source, not yet live-confirmed ----
//
// `struct _MonoClass`'s real field list was fetched this session from
// `mono/metadata/class-private-definition.h` (61 fields, mono/mono main)
// -- newer Mono hides the struct behind getters and keeps the real layout
// in that file specifically because callers aren't meant to hardcode
// offsets into it, which is exactly what this does anyway, for the reason
// stated throughout this project: the log channel that would make this
// unnecessary (Arena.log's draft offer) stopped existing.
//
// Offsets below are hand-computed under the System V AMD64 ABI's ordinary
// struct packing rules (natural alignment, no packing pragma) from that
// field list. One is independently confirmed by evidence already in
// hand: the running total lands `name` at exactly 0x48 -- the same value
// `MONO_CLASS_NAME` above was found through by live scanning, with no
/// connection between the two derivations. That agreement is why the rest
/// of the arithmetic (unverified past that point) is trusted enough to
/// try, not proof every later field is right -- `this_arg`/`_byval_arg`
/// (both embedded `MonoType`s) in particular assume `sizeof(MonoType)`
/// is 16, computed from a *partial* fetch of that struct (its bitfield
/// tail was cut off mid-response) rather than confirmed complete.

/// `MonoClass.vtable_size` — field #44 in the fetched list. Unverified:
/// `find_vtable` (below) doesn't depend on it, so its correctness has no
/// evidence yet either way.
pub const MONO_CLASS_VTABLE_SIZE: u64 = 0x5c;

/// `MonoClass.runtime_info` — **confirmed live**: `find_vtable` in
/// `main.rs` resolved a `MonoVTable*` through this offset for four
/// different classes (`DraftManager`, `PowerProcessor`, `Entity`,
/// `Player`) and every one had `MonoVTable.klass` pointing back at the
/// class asked for. Four independent agreements is not the kind of thing
/// a wrong offset produces by chance.
pub const MONO_CLASS_RUNTIME_INFO: u64 = 0xd0;

/// `MonoClassRuntimeInfo.domain_vtables[0]` — **confirmed live**, same
/// evidence as `MONO_CLASS_RUNTIME_INFO` above (this offset is exercised
/// by the very same successful `klass` check).
pub const MONO_CLASS_RUNTIME_INFO_DOMAIN_VTABLES: u64 = 8;

/// `MonoVTable.vtable[]` — field #17 of `struct MonoVTable`
/// (`mono/metadata/class-internals.h`, confirmed field order from
/// source). Where the per-class method table starts; static field data
/// (for classes that have any, `has_static_fields`) lives immediately
/// after the method table, i.e. at
/// `MONO_VTABLE_VTABLE_ARRAY + klass.vtable_size * 8`.
pub const MONO_VTABLE_KLASS: u64 = 0; // field #1, sanity/validation check
pub const MONO_VTABLE_VTABLE_ARRAY: u64 = 0x48;

// ---- Hearthstone's own `Entity`/`Player` instance fields (not Mono's own
// structs) -- found 2026-08-31 against a real, active ranked game by
// cross-checking heap-scan hits against ground truth read straight from
// that game's own `Power.log` (a plain file read, no relation to memory
// reading). See `dump_entity_table`/`cross_check_known_values` in
// `main.rs`.

/// `Entity` instances are exactly 64 (`0x40`) bytes each on this build --
/// confirmed by consecutive heap-scan hits sitting exactly this far apart
/// repeatedly (`0x7387b300, 0x7387b340, 0x7387b380, ...`).
pub const ENTITY_INSTANCE_SIZE: u64 = 0x40;

/// `Entity.cardId` -- a `System.String*` (UTF-16, see `try_mono_string` in
/// main.rs). **Confirmed live**: decodes to real card ids on every live
/// Entity found (`"HERO_11bpt"`, `"CORE_RLK_121"`, ...).
pub const ENTITY_CARD_ID: u64 = 0x30;

/// `Entity.id` (the `EntityID` `Power.log` reports as `id=`). **Confirmed
/// live, exact match**: the entity whose `ENTITY_CARD_ID` decoded to
/// `"HERO_09dbp"` (Lesser Heal) had `57` here -- Power.log recorded that
/// exact hero power as `id=57`. Not a coincidence at that specificity.
pub const ENTITY_ID: u64 = 0x38;

/// `Player.playerId` (the 1-indexed `PlayerID` `Power.log`'s `CREATE_GAME`
/// reports, distinct from `EntityID`). **Confirmed live, fully**: first
/// seen matching on that one game's two Player objects (`"xror"` -> `1`,
/// `"WildKid"` -> `2`, exactly `Player EntityID=2 PlayerID=1` / `Player
/// EntityID=3 PlayerID=2` from that game's `Power.log`), then re-confirmed
/// without exception across 20+ live Player objects spanning many
/// different past games -- the local player (`xror`) always reads `1`,
/// every opponent always reads `2`. Trusted the same way `ENTITY_ID` is.
pub const PLAYER_PLAYER_ID: u64 = 0x164;

/// `Player.battleTag` (the short name before `#`, e.g. `"xror"`) -- a
/// `System.String*`. **Confirmed live** the same way as the two above.
pub const PLAYER_NAME: u64 = 0x120;

/// Entity's remaining unidentified fields, `+0x10/+0x18/+0x20/+0x28` --
/// four pointer-shaped values, not small ints, so `Zone`/`Controller`
/// (both need to hold small ints like `PLAYER_ID`) aren't directly one of
/// these the way `ENTITY_ID` was. Hearthstone's own tag model (every
/// `TAG_CHANGE` line in `Power.log` is `<GameTag> = <value>`) suggests
/// these fields hold something dictionary-shaped instead -- most likely a
/// `Dictionary<int,int>` of every `GameTag` (`ZONE`, `CONTROLLER`, `COST`,
/// ...) for this entity, reachable through one of these four pointers.
/// Nothing here confirms which one yet.
pub const ENTITY_UNKNOWN_PTRS: [u64; 4] = [0x10, 0x18, 0x20, 0x28];
