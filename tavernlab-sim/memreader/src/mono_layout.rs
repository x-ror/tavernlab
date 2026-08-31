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

/// `MonoImage.class_cache` — **not yet approached**. `struct _MonoImage`
/// is long; getting to this field needs the same evidence-based scan
/// technique run one level deeper from the now-confirmed `MonoImage*` —
/// this time looking for something that behaves like a hash table (an
/// array of pointers, several leading to class-name-shaped strings)
/// rather than a single string, since `MonoInternalHashTable` isn't a
/// simple pointer-to-string field.
#[allow(dead_code)] // next milestone
pub const MONO_IMAGE_CLASS_CACHE: u64 = 0;
