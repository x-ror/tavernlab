//! Byte offsets into Mono's internal runtime structs, for x86-64.
//!
//! **Read this before trusting any of these numbers.** Every constant here
//! falls into one of two very different confidence tiers, and the comment
//! above it says which:
//!
//! - **Confirmed field order.** Fetched from the public, MIT-licensed Mono
//!   runtime source (`mono/mono`, `main` branch) during this session —
//!   `struct _MonoAssembly` in `mono/metadata/metadata-internals.h`. The
//!   *order* fields appear in is as declared there.
//!
//! - **Unverified.** Anything about `MonoDomain`'s layout, and the exact
//!   size of the embedded `MonoAssemblyName` struct inside `MonoAssembly`.
//!   The headers that define both (`domain-internals.h`, and whichever
//!   header carries `MonoAssemblyName`) could not be fetched this
//!   session — repeated attempts against both the upstream `mono/mono` and
//!   the more relevant `Unity-Technologies/mono` fork were refused by this
//!   session's own safety layer, for reasons unrelated to whether the
//!   fetch itself was safe. These are exactly the numbers `--deep` will
//!   most likely need corrected first — see `memreader/README.md`.
//!
//! A second, independent source of drift sits on top of all this: Unity's
//! embedded Mono (`mono-2.0-bdwgc.dll`, "MonoBleedingEdge") forked from
//! upstream years ago and does not track it field-for-field. Treat every
//! offset here — confirmed tier included — as "this is what the struct
//! looked like in the branch it was fetched from," not "this is what
//! Hearthstone's binary has," until it is checked against a live read.

/// `struct _MonoAssembly` on x86-64, per the confirmed field order:
/// `gint32 ref_count; char *basedir; MonoAssemblyName aname; MonoImage
/// *image; ...`. `ref_count` is 4 bytes, padded to 8 for the pointer that
/// follows (natural alignment, no packing pragma in the source) — so
/// `basedir` sits at 8, and `aname` starts at 16.
pub const MONO_ASSEMBLY_ANAME: u64 = 16;

/// `MonoAssemblyName.name` — reasonably likely to be the *first* field of
/// that struct (a `char *` display name is the conventional lead field
/// across every Mono-derived struct of this kind this session has seen),
/// but the struct's own definition was never fetched, so this is inferred
/// from convention, not read from source. Offset from the start of
/// `aname`, i.e. from `MONO_ASSEMBLY_ANAME`.
pub const MONO_ASSEMBLY_NAME_NAME_FIELD: u64 = 0;
/// Convenience: `aname.name` as an absolute offset from the `MonoAssembly*`.
pub const MONO_ASSEMBLY_NAME: u64 = MONO_ASSEMBLY_ANAME + MONO_ASSEMBLY_NAME_NAME_FIELD;

/// `MonoAssembly.image` — sits right after `aname`, so this needs
/// `sizeof(MonoAssemblyName)`, which is **not known**: the struct's field
/// list was never fetched. 96 is a commonly-cited figure for this struct
/// on 64-bit Mono elsewhere, carried here as a first guess, not a
/// confirmed value — if `--deep` prints a name correctly via
/// `MONO_ASSEMBLY_NAME` but then reads garbage as the image pointer, this
/// is the constant to walk up or down in 8-byte steps against a real
/// process until the printed `MonoImage*` looks like a plausible pointer.
pub const MONO_ASSEMBLY_ANAME_SIZE_GUESS: u64 = 96;
pub const MONO_ASSEMBLY_IMAGE: u64 = MONO_ASSEMBLY_ANAME + MONO_ASSEMBLY_ANAME_SIZE_GUESS;

/// `MonoImage.class_cache` — **not computed**. `struct _MonoImage` is
/// long; getting to this field means summing the size of everything before
/// it in the confirmed field list handed to whoever continues this (see
/// the design notes in the project chat), which wasn't done this session.
/// Placeholder so the crate compiles and so `--deep`'s next step is
/// visibly a TODO rather than a silent wrong number.
#[allow(dead_code)] // wired up once MonoImage's field sizes are actually computed
pub const MONO_IMAGE_CLASS_CACHE: u64 = 0;

/// `MonoDomain.domain_assemblies` — **unverified**. Carried over from an
/// old public 32-bit HearthMirror fork found this session; that is *not*
/// evidence it holds for a 64-bit build of a materially different, much
/// newer Mono runtime. This is the first number to expect `--deep` to get
/// wrong, and the first one worth re-deriving properly (ideally by finally
/// getting `domain-internals.h`'s real content, outside whatever blocked
/// it here) rather than brute-force guessing against the live process.
pub const MONO_DOMAIN_DOMAIN_ASSEMBLIES: u64 = 0x6c;
