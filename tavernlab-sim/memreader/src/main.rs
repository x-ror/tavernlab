//! A read-only Mono memory reader for a locally running Hearthstone.exe,
//! built as a companion to `tavernsim watch` — see
//! `tavernlab-sim/memreader/README.md` for what this is, why it exists, and
//! how to run it. Read that before this file; it explains the legal/design
//! posture this has to respect (docs/DESIGN.md "Правовий режим").
//!
//! Nothing here executes code in the target process. Every step is either
//! reading a file on disk (the DLL itself, to parse its own PE headers) or
//! `process_vm_readv` against the running process (a plain read syscall,
//! the same one `/proc/PID/mem` gives you, just batched). The one place
//! this looks at machine code at all is `decode_root_domain_thunk`, and it
//! only *decodes* a handful of bytes to find an address -- it never runs
//! them.
//!
//! This is a spike, not a finished tool: the PID/module-discovery and PE
//! parsing pieces are mechanical and should just work; the Mono struct
//! offsets in `mono_layout` are a best estimate for x86-64 computed from
//! the public Mono runtime source, unverified against this exact build.
//! Expect the `--deep` walk to need correction — that's what `--probe`
//! and the raw hex dumps are for.

use std::env;
use std::fs;
use std::process::exit;

mod procfs;
mod procmem;
mod pe;
mod mono_layout;

use procmem::Remote;

fn main() {
    let args: Vec<String> = env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("--probe");

    let pid = match procfs::find_pid_by_name("Hearthstone.exe") {
        Some(p) => p,
        None => {
            eprintln!(
                "не знайшов процес Hearthstone.exe. Він має бути запущений \
                 (перевірте `ps aux | grep -i hearthstone`)."
            );
            exit(1);
        }
    };
    println!("PID Hearthstone.exe: {pid}");

    let maps = match procfs::read_maps(pid) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "не зміг прочитати /proc/{pid}/maps: {e}. Найімовірніша причина \
                 — потрібні права: запустіть memreader тим самим користувачем, \
                 що й Hearthstone.exe (не root, не sudo)."
            );
            exit(1);
        }
    };

    let dll = match maps.iter().find(|m| m.pathname.ends_with("mono-2.0-bdwgc.dll")) {
        Some(m) => m,
        None => {
            eprintln!(
                "не знайшов mono-2.0-bdwgc.dll серед мапованих модулів процесу. \
                 Або клієнт ще не догрузився, або це інша версія Unity/Mono — \
                 у такому разі скиньте мені вміст `grep -i mono /proc/{pid}/maps`."
            );
            exit(1);
        }
    };
    println!(
        "mono-2.0-bdwgc.dll: base=0x{:x} (файл: {})",
        dll.start, dll.pathname
    );

    // The lowest mapped region for the module's own path is the header
    // page (offset 0 into the file), which is what PE's RVAs are relative
    // to for a normally loaded image — same assumption HearthMirror's own
    // `ProcessView` makes for the remote case. We get to check this one
    // for free: parse the *file* on disk (a plain read, not a process
    // read) and confirm it starts with the `MZ`/`PE` signatures we expect.
    let file_bytes = match fs::read(&dll.pathname) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("не зміг прочитати файл {}: {e}", dll.pathname);
            exit(1);
        }
    };
    let exports = match pe::parse_exports(&file_bytes) {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("не розпарсив PE-заголовки {}: {msg}", dll.pathname);
            exit(1);
        }
    };
    println!("PE розпарсено, {} експортів знайдено", exports.len());

    let Some(&rva) = exports.get("mono_get_root_domain") else {
        eprintln!(
            "у експортах немає mono_get_root_domain. Це або зовсім інший \
             Mono runtime, або назва символу відрізняється в цьому білді — \
             скиньте мені список експортів, що містять слово \"domain\"."
        );
        for name in exports.keys().filter(|n| n.to_lowercase().contains("domain")) {
            eprintln!("  {name}");
        }
        exit(1);
    };
    let func_addr = dll.start + rva as u64;
    println!("mono_get_root_domain: RVA=0x{rva:x} -> адреса в процесі 0x{func_addr:x}");

    let remote = Remote::new(pid);
    let prologue = match remote.read(func_addr, 16) {
        Some(b) => b,
        None => {
            eprintln!(
                "process_vm_readv не зміг прочитати 16 байтів з 0x{func_addr:x}. \
                 Перевірте права (той самий користувач, CAP_SYS_PTRACE або \
                 однаковий uid), і що ptrace_scope не забороняє це \
                 (`cat /proc/sys/kernel/yama/ptrace_scope`; 0 або 1 мають \
                 працювати для процесу того самого користувача)."
            );
            exit(1);
        }
    };
    println!("перші 16 байтів функції: {}", hex(&prologue));

    let Some(domain_ptr_addr) = decode_root_domain_thunk(func_addr, &prologue) else {
        eprintln!(
            "не впізнав опкод у прологу — очікував `48 8b 05 xx xx xx xx c3` \
             (mov rax, [rip+disp32]; ret), можливо з проміжним jmp-thunk. \
             Скиньте мені рядок з перших 16 байтів вище, і я підправлю \
             декодер під цей конкретний білд."
        );
        exit(1);
    };
    println!("адреса глобальної змінної root domain: 0x{domain_ptr_addr:x}");

    let Some(domain_bytes) = remote.read(domain_ptr_addr, 8) else {
        eprintln!("не зміг прочитати 8 байтів з 0x{domain_ptr_addr:x}");
        exit(1);
    };
    let domain_addr = u64::from_le_bytes(domain_bytes.try_into().unwrap());
    println!("MonoDomain*: 0x{domain_addr:x}");

    if domain_addr == 0 {
        eprintln!(
            "прочитана адреса MonoDomain — нуль. Або гра ще не запустила \
             жодної гри/сцени, де рушій уже crank-нув домен (малоймовірно, \
             він живий з самого старту процесу), або декодер прологу вище \
             вказує не туди. Спробуйте ще раз за кілька секунд; якщо нуль \
             стабільно — скиньте мені весь вивід цієї команди."
        );
        exit(1);
    }

    if mode == "--probe" {
        println!(
            "\n--probe завершено успішно: PID, модуль, root domain pointer — \
             усе знайдено. Це основа, яка не залежить від точних офсетів \
             Mono-структур. Надішліть мені весь вивід вище, і я підготую \
             --deep крок (MonoDomain -> Assembly-CSharp -> класи) під нього."
        );
        return;
    }

    // domain_assemblies and MonoAssembly.aname.name are confirmed (see
    // mono_layout.rs); walk the real list, then chase Assembly-CSharp's
    // image the same evidence-based way that offset was found.
    if let Some(csharp) = walk_assembly_list(&remote, domain_addr) {
        if let Some(image) = read_image(&remote, csharp) {
            if mode == "--scan-classes" {
                scan_for_class_table(&remote, image);
            } else {
                let classes = dump_class_names(&remote, image);
                let resolved = find_singletons(&remote, &classes);
                for target in ["Entity", "Player"] {
                    if let Some(&(_, _, vtable)) =
                        resolved.iter().find(|(n, _, _)| n == target)
                    {
                        let hits = scan_heap_for_class(&remote, pid, target, vtable);
                        println!(
                            "  ... і ще {} (показано перші 20 адрес вище)",
                            hits.len().saturating_sub(20)
                        );
                        for &addr in hits.iter().take(3) {
                            dump_object_fields(&remote, addr, target);
                        }
                        if target == "Entity" && !hits.is_empty() {
                            dump_entity_table(&remote, &hits);
                        }
                        if target == "Player" && !hits.is_empty() {
                            dump_player_table(&remote, &hits);
                        }
                        if !hits.is_empty() {
                            println!(
                                "\nСкиньте мені весь вивід. \"рядки\" — якщо там \
                                 щось на кшталт CardID (\"CS2_182\") — це найкращий \
                                 доказ; \"малі числа\" — кандидати на entity id, \
                                 менш надійні, дивимось разом."
                            );
                        }
                    }
                }
            }
        }
    }
}

/// x86-64 Mono typically compiles `mono_get_root_domain` down to reading a
/// static global through a RIP-relative load and returning it:
/// `48 8b 05 <disp32>  c3` (`mov rax, [rip+disp32]; ret`). The absolute
/// address of the global is the address *after* that instruction (RIP at
/// the time it executes) plus the displacement. A `jmp rel32` thunk
/// (`e9 <rel32>`) is followed once in case the export is a trampoline
/// rather than the function body itself.
fn decode_root_domain_thunk(func_addr: u64, bytes: &[u8]) -> Option<u64> {
    if bytes.len() >= 5 && bytes[0] == 0xe9 {
        let rel = i32::from_le_bytes(bytes[1..5].try_into().ok()?);
        // We only have 16 bytes of the *target*'s prologue available to
        // the caller in this simple version; a jmp thunk needs a second
        // read, which the caller doesn't do yet. Reported rather than
        // silently mishandled.
        eprintln!(
            "(нотатка: перший байт — 0xe9, jmp-thunk на 0x{:x}; цей декодер \
             поки не читає ціль другим read'ом — якщо основний патерн не \
             підійде, це перше, що варто додати)",
            (func_addr as i64 + 5 + rel as i64) as u64
        );
    }
    if bytes.len() >= 8 && bytes[0] == 0x48 && bytes[1] == 0x8b && bytes[2] == 0x05 {
        let disp = i32::from_le_bytes(bytes[3..7].try_into().ok()?);
        let next_insn = func_addr + 7;
        return Some((next_insn as i64 + disp as i64) as u64);
    }
    None
}

/// A pointer value is "plausible" if it is non-null and sits in the
/// canonical low half of the 64-bit address space (real user-mode
/// pointers on Linux/Wine never set the high bits) -- cheap enough to
/// filter obvious non-pointers (small integers, flags, counts) out of a
/// brute-force scan without needing to know a single real offset first.
fn plausible_ptr(v: u64) -> bool {
    v != 0 && v < 0x0000_8000_0000_0000
}

/// Whether `bytes` looks like a short, printable identifier -- an assembly
/// or class name, not binary data. Deliberately strict: this is what kept
/// the domain_assemblies and MonoAssembly.image scans (both now resolved,
/// see mono_layout.rs) from reporting noise, and does the same job for
/// `scan_for_class_table` below.
///
/// This only ever matches single-byte (Latin-1/ASCII) C strings -- which is
/// exactly right for `MonoClass`/`MonoAssembly` names (those are plain
/// `char*`, confirmed by every offset in `mono_layout.rs`), but wrong for
/// an actual C# `System.String` field, which Mono stores as UTF-16. That
/// mismatch is almost certainly why the Entity/Player instance dumps came
/// back empty or with 4-character noise ("HA6b", "8POs"): a `MonoString`
/// pointer's bytes just don't look like this. `try_mono_string` below
/// handles that shape instead.
fn looks_like_name(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0)?;
    if !(2..48).contains(&end) {
        return None;
    }
    let s = std::str::from_utf8(&bytes[..end]).ok()?;
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '`'))
        .then(|| s.to_string())
}

/// Decode `ptr` as a `System.String` (`MonoString`), on the well-documented
/// layout every Mono-introspection tool (HearthMirror, MemorySharp, ...)
/// relies on: a 16-byte `MonoObject` header (`vtable*`, `sync*`), then a
/// `gint32 length` field, then `length` UTF-16LE code units, then a
/// trailing null. On x86-64 that puts `length` at `+0x10` and the char data
/// at `+0x14` -- no padding gap, since `gint32` and `gunichar2[]` are both
/// already aligned there. This is a stronger, more specific check than
/// `looks_like_name`: it trusts the length field's own value (not a null
/// terminator search) to know how many UTF-16 units to read, then requires
/// every decoded character to be printable ASCII -- garbage data at `+0x10`
/// almost never happens to look like both a plausible small length *and*
/// an all-printable string that follows it.
fn try_mono_string(remote: &Remote, ptr: u64) -> Option<String> {
    if !plausible_ptr(ptr) {
        return None;
    }
    let len_bytes = remote.read(ptr + 0x10, 4)?;
    let len = i32::from_le_bytes(len_bytes.try_into().unwrap());
    if !(1..=64).contains(&len) {
        return None;
    }
    let char_bytes = remote.read(ptr + 0x14, len as usize * 2)?;
    let units: Vec<u16> = char_bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let s = String::from_utf16(&units).ok()?;
    s.chars()
        .all(|c| c.is_ascii_graphic() || c == ' ')
        .then(|| s)
}

/// Walk the confirmed `domain_assemblies` `GList` (see `mono_layout.rs`)
/// and print every assembly name found. Where the scan that discovered the
/// offset had one piece of evidence (`"mscorlib"`), this has dozens —
/// every assembly Hearthstone loads — which is what makes this a
/// confirmation pass rather than another guess.
fn walk_assembly_list(remote: &Remote, domain_addr: u64) -> Option<u64> {
    println!("\n--deep: MonoDomain -> domain_assemblies (офсет підтверджено скануванням)");
    let head_addr = domain_addr + mono_layout::MONO_DOMAIN_DOMAIN_ASSEMBLIES;
    let Some(head_bytes) = remote.read(head_addr, 8) else {
        eprintln!("не зміг прочитати domain_assemblies за 0x{head_addr:x}");
        return None;
    };
    let mut node = u64::from_le_bytes(head_bytes.try_into().unwrap());
    let mut hops = 0;
    let mut csharp_data = None;
    while plausible_ptr(node) && hops < 128 {
        hops += 1;
        let Some(pair) = remote.read(node, 16) else { break };
        let data = u64::from_le_bytes(pair[0..8].try_into().unwrap());
        let next = u64::from_le_bytes(pair[8..16].try_into().unwrap());
        if plausible_ptr(data) {
            let name_ptr_addr = data + mono_layout::MONO_ASSEMBLY_NAME;
            if let Some(name_ptr_bytes) = remote.read(name_ptr_addr, 8) {
                let name_ptr = u64::from_le_bytes(name_ptr_bytes.try_into().unwrap());
                if let Some(name) = read_cstring(remote, name_ptr, 64) {
                    println!("  assembly: {name}");
                    if name == "Assembly-CSharp" {
                        csharp_data = Some(data);
                    }
                }
            }
        }
        node = next;
    }
    println!("({hops} вузлів пройдено)");
    if csharp_data.is_none() {
        eprintln!(
            "серед {hops} вузлів немає \"Assembly-CSharp\" — або гра ще не \
             завантажила ігрову збірку (спробуйте після заходу в бій чи \
             драфт), або domain_assemblies веде не туди, куди здається \
             (mscorlib збігся випадково). Скиньте мені весь список."
        );
    }
    csharp_data
}

/// `MonoAssembly.image` is confirmed (`mono_layout.rs`), so this reads it
/// directly and validates the read against the name/filename pair the
/// scan that found the offset also confirmed — a cheap sanity check that
/// costs nothing and catches "the offset was right last run but this
/// build/session moved something" immediately rather than silently.
fn read_image(remote: &Remote, assembly_data: u64) -> Option<u64> {
    println!("\n--deep: MonoAssembly(Assembly-CSharp) -> image (офсет підтверджено)");
    let addr = assembly_data + mono_layout::MONO_ASSEMBLY_IMAGE;
    let image = u64::from_le_bytes(remote.read(addr, 8)?.try_into().unwrap());
    if !plausible_ptr(image) {
        eprintln!("MonoImage* за 0x{addr:x} не схожий на вказівник (0x{image:x})");
        return None;
    }
    let name = read_cstring(remote, {
        let p = remote.read(image + mono_layout::MONO_IMAGE_NAME, 8)?;
        u64::from_le_bytes(p.try_into().unwrap())
    }, 64);
    let filename = read_cstring(remote, {
        let p = remote.read(image + mono_layout::MONO_IMAGE_FILENAME, 8)?;
        u64::from_le_bytes(p.try_into().unwrap())
    }, 64);
    println!(
        "MonoImage*: 0x{image:x}  name={:?}  filename={:?}",
        name, filename
    );
    if name.as_deref() != Some("Assembly-CSharp") {
        eprintln!(
            "name не дорівнює \"Assembly-CSharp\" — офсет MONO_ASSEMBLY_IMAGE \
             у mono_layout.rs, можливо, застарів для цього процесу; надішліть \
             мені цей вивід."
        );
    }
    Some(image)
}

/// Names likely to belong to Hearthstone's own game-state classes rather
/// than Unity/BCL plumbing -- printed with a marker so a long class list
/// doesn't bury the entries worth following up on. Not an exhaustive or
/// authoritative list, just the ones this session's chat named as the
/// live-tracking target (turn, hand, board, plays) plus the Arena draft
/// class named earlier -- adjust freely once real names are seen.
const INTERESTING_SUBSTRINGS: &[&str] = &[
    "GameState",
    "DraftManager",
    "Entity",
    "Player",
    "Board",
    "Network.Game",
    "PowerProcessor",
    "ZoneMgr",
];

/// `MonoImage.class_cache` is a `MonoInternalHashTable`, not a single
/// pointer to a string -- so this is a different shape of evidence-based
/// scan than `walk_assembly_list`/`read_image` above, but the same idea:
/// look for a bucket array (many consecutive plausible pointers) whose
/// entries lead to short, name-shaped strings, rather than trust a single
/// guessed offset for both the table's location in `MonoImage` and a
/// class's name field within `MonoClass`.
///
/// This is the widest, slowest scan in the tool by a wide margin -- expect
/// it to take real seconds, not the near-instant reply of the scans
/// before it.
///
/// The first version of this scan (kept a record of in the commit
/// history) took the *first* name-shaped hit per bucket at whichever
/// offset produced one, and got fooled: `MonoImage+0x48`/`+0x58` turned
/// out to be the `references` array (each entry a `MonoAssembly*`, hit at
/// `+0x10` -- the very offset `MONO_ASSEMBLY_NAME` already confirms), and
/// `MonoImage+0x8` mixed real class names with method names from entirely
/// different structs that happened to have *some* plausible pointer at
/// *some* tried offset. A real class table is structurally homogeneous:
/// almost every bucket is a `MonoClass*`, so the name should sit at the
/// *same* offset across nearly all of them. This version requires that
/// dominant-offset agreement before it calls something a match, which is
/// what actually separates a real table from scan noise.
fn scan_for_class_table(remote: &Remote, image: u64) {
    println!("\n--deep: MonoImage.class_cache (сканую — найширший і найповільніший крок)");
    const SEARCH_RANGE: u64 = 0x6000;
    const BUCKETS_TO_PROBE: usize = 256;
    const NAME_OFFSETS: &[u64] = &[0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80];
    const MIN_DENSITY: usize = 6;
    const MAX_TABLES_PROBED: usize = 80;
    /// A dominant offset needs at least this many agreeing hits, and to
    /// account for at least this fraction of everything found, before the
    /// table counts as structurally consistent rather than noise.
    const MIN_AGREEING_HITS: usize = 8;
    const MIN_AGREEMENT_FRACTION: f64 = 0.6;

    let mut tables_probed = 0;
    let mut strong_matches = 0;
    for table_off in (0..SEARCH_RANGE).step_by(8) {
        if tables_probed >= MAX_TABLES_PROBED {
            println!("  (зупиняюсь на {MAX_TABLES_PROBED} таблицях-кандидатах)");
            break;
        }
        let Some(ptr_bytes) = remote.read(image + table_off, 8) else { continue };
        let table_ptr = u64::from_le_bytes(ptr_bytes.try_into().unwrap());
        if !plausible_ptr(table_ptr) {
            continue;
        }
        let Some(bucket_bytes) = remote.read(table_ptr, BUCKETS_TO_PROBE * 8) else { continue };
        let buckets: Vec<u64> = bucket_bytes
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let density = buckets.iter().filter(|&&b| plausible_ptr(b)).count();
        if density < MIN_DENSITY {
            continue;
        }
        tables_probed += 1;

        // Every name found, per bucket, at every offset that produced one
        // -- not just the first -- so the offset that dominates can be
        // picked out afterwards instead of committed to per-bucket early.
        let mut by_offset: std::collections::HashMap<u64, Vec<String>> = Default::default();
        let mut checked = 0;
        for &bucket in buckets.iter().filter(|&&b| plausible_ptr(b)) {
            checked += 1;
            if checked > 200 {
                break;
            }
            for &name_off in NAME_OFFSETS {
                let Some(np) = remote.read(bucket + name_off, 8) else { continue };
                let name_ptr = u64::from_le_bytes(np.try_into().unwrap());
                if !plausible_ptr(name_ptr) {
                    continue;
                }
                let Some(str_bytes) = remote.read(name_ptr, 64) else { continue };
                if let Some(name) = looks_like_name(&str_bytes) {
                    by_offset.entry(name_off).or_default().push(name);
                }
            }
        }
        let total_hits: usize = by_offset.values().map(Vec::len).sum();
        if total_hits == 0 {
            continue;
        }
        let Some((&dom_off, dom_names)) = by_offset.iter().max_by_key(|(_, v)| v.len()) else {
            continue;
        };
        let agreement = dom_names.len() as f64 / total_hits as f64;
        if dom_names.len() < MIN_AGREEING_HITS || agreement < MIN_AGREEMENT_FRACTION {
            continue; // structurally inconsistent -- almost certainly noise
        }

        strong_matches += 1;
        println!(
            "  таблиця-кандидат: MonoImage+0x{table_off:x} -> 0x{table_ptr:x} \
             ({density}/{BUCKETS_TO_PROBE} заповнено, {}/{total_hits} узгоджено на +0x{dom_off:x})",
            dom_names.len()
        );
        let interesting: Vec<&String> = dom_names
            .iter()
            .filter(|n| INTERESTING_SUBSTRINGS.iter().any(|kw| n.contains(kw)))
            .collect();
        if !interesting.is_empty() {
            println!("    !!! знайдено цікаві назви:");
            for n in &interesting {
                println!("      \"{n}\"");
            }
        }
        println!("    приклади ({} узгоджених):", dom_names.len());
        for n in dom_names.iter().take(15) {
            println!("      \"{n}\"");
        }
    }
    if strong_matches == 0 {
        eprintln!(
            "{tables_probed} таблиць з достатньою щільністю переглянуто, жодна не \
             показала однорідного офсету назви — тобто жодна не схожа на \
             MonoInternalHashTable класів. Скиньте мені весь вивід; наступний \
             крок — розширити SEARCH_RANGE або спробувати менш строгий поріг \
             узгодження."
        );
        return;
    }
    println!(
        "\nЗнайдено {strong_matches} однорідних таблиць. Скиньте мені весь блок \
         вище. Якщо серед \"!!! знайдено цікаві назви\" є GameState/Entity/\
         Player — саме там і продовжимо: наступний крок — прочитати поля \
         цього MonoClass (vtable/статичні поля, пошук екземпляра гри, що \
         зараз йде)."
    );
}

/// `MonoImage.class_cache`'s table and `MonoClass.name` are confirmed
/// (`mono_layout.rs`) — this is the payoff: every class name
/// `Assembly-CSharp` actually has, not the 256-entry sample the scan that
/// found the offsets was capped to. The 256/256 density seen there means
/// the real table is very likely bigger (a Mono hash table that full is
/// due for a resize), so this tries progressively larger reads and keeps
/// the largest one that actually came back whole.
/// Returns every `(name, MonoClass*)` pair the class cache gives up —
/// unlike the first version of this function, the pointer is kept, not
/// just the name, because the next step (finding a live singleton
/// instance) needs it.
fn dump_class_names(remote: &Remote, image: u64) -> Vec<(String, u64)> {
    println!("\n--deep: MonoImage.class_cache -> усі класи Assembly-CSharp (офсети підтверджено)");
    let table_ptr_addr = image + mono_layout::MONO_IMAGE_CLASS_CACHE_TABLE;
    let Some(tp) = remote.read(table_ptr_addr, 8) else {
        eprintln!("не зміг прочитати вказівник таблиці за 0x{table_ptr_addr:x}");
        return Vec::new();
    };
    let table_ptr = u64::from_le_bytes(tp.try_into().unwrap());
    if !plausible_ptr(table_ptr) {
        eprintln!("0x{table_ptr:x} не схоже на вказівник — офсет міг застаріти");
        return Vec::new();
    }

    const CANDIDATE_SIZES: &[usize] = &[65536, 32768, 16384, 8192, 4096, 2048, 1024, 512, 256];
    let mut buckets: Vec<u64> = Vec::new();
    for &n in CANDIDATE_SIZES {
        if let Some(bytes) = remote.read(table_ptr, n * 8) {
            buckets = bytes
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                .collect();
            println!("  прочитано таблицю з {n} бакетів");
            break;
        }
    }
    if buckets.is_empty() {
        eprintln!("не зміг прочитати жоден розмір таблиці з 0x{table_ptr:x}");
        return Vec::new();
    }

    let mut classes: Vec<(String, u64)> = Vec::new();
    for &bucket in buckets.iter().filter(|&&b| plausible_ptr(b)) {
        let Some(np) = remote.read(bucket + mono_layout::MONO_CLASS_NAME, 8) else { continue };
        let name_ptr = u64::from_le_bytes(np.try_into().unwrap());
        let Some(name) = read_cstring(remote, name_ptr, 96) else { continue };
        if name.is_empty() {
            continue;
        }
        classes.push((name, bucket));
    }
    println!(
        "  {} непорожніх бакетів, {} з них дали назву класу",
        buckets.iter().filter(|&&b| plausible_ptr(b)).count(),
        classes.len()
    );
    let interesting: Vec<&str> = classes
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| INTERESTING_SUBSTRINGS.iter().any(|kw| n.contains(kw)))
        .collect();
    if !interesting.is_empty() {
        println!("\n  !!! класи, що збігаються з ключовими словами:");
        for n in &interesting {
            println!("    {n}");
        }
    }
    classes
}

/// Given a `MonoClass*`, find `MonoVTable*` for it in whichever domain
/// index actually has one, validated by checking `MonoVTable.klass`
/// points back at the class we asked for — not just trusting index 0.
fn find_vtable(remote: &Remote, class_ptr: u64) -> Option<u64> {
    let ri_bytes = remote.read(class_ptr + mono_layout::MONO_CLASS_RUNTIME_INFO, 8)?;
    let runtime_info = u64::from_le_bytes(ri_bytes.try_into().unwrap());
    if !plausible_ptr(runtime_info) {
        return None; // class exists but was never touched by the runtime -- no vtable yet
    }
    for slot in 0..4u64 {
        let addr = runtime_info
            + mono_layout::MONO_CLASS_RUNTIME_INFO_DOMAIN_VTABLES
            + slot * 8;
        let Some(vb) = remote.read(addr, 8) else { continue };
        let vtable = u64::from_le_bytes(vb.try_into().unwrap());
        if !plausible_ptr(vtable) {
            continue;
        }
        let Some(kb) = remote.read(vtable + mono_layout::MONO_VTABLE_KLASS, 8) else { continue };
        let klass = u64::from_le_bytes(kb.try_into().unwrap());
        if klass == class_ptr {
            return Some(vtable);
        }
    }
    None
}

/// Look for a static field, anywhere in the class's static-data blob,
/// whose value is a pointer to an object of *this same class* — the
/// signature every Mono object has is its own first 8 bytes being a
/// `MonoVTable*`, so `object -> vtable -> klass == class_ptr` is checked
/// for every candidate rather than needing to know which field or its
/// name. This is the same "known shape, unknown offset" trick as every
/// other scan in this file, just one level further down.
fn find_self_typed_static(remote: &Remote, class_ptr: u64, vtable: u64) -> Vec<u64> {
    let vtable_size = remote
        .read(class_ptr + mono_layout::MONO_CLASS_VTABLE_SIZE, 4)
        .map(|vs| i32::from_le_bytes(vs.try_into().unwrap()).max(0) as u64)
        .unwrap_or(0);
    println!(
        "    vtable_size (MonoClass+0x{:x}) = {vtable_size}",
        mono_layout::MONO_CLASS_VTABLE_SIZE
    );

    // Two hypotheses scanned at once, since vtable_size's correctness is
    // itself unverified: (a) static data starts right after the method
    // table, at vtable[] + vtable_size*8, the textbook Mono layout; (b) it
    // sits somewhere in a wider window from the vtable array's own start,
    // in case vtable_size or MONO_VTABLE_VTABLE_ARRAY is off and the real
    // data is nearby anyway (Mono usually allocates the vtable and its
    // trailing data as one block).
    let computed = vtable + mono_layout::MONO_VTABLE_VTABLE_ARRAY + vtable_size * 8;
    let wide_start = vtable + mono_layout::MONO_VTABLE_VTABLE_ARRAY;
    const WIDE_LEN: usize = 0x3000;
    println!(
        "    обчислений старт статичних даних: 0x{computed:x}; ширший діапазон: \
         0x{wide_start:x}..+0x{WIDE_LEN:x}"
    );

    let Some(blob) = remote.read(wide_start, WIDE_LEN) else {
        println!("    не зміг прочитати навіть ширший діапазон з 0x{wide_start:x}");
        return Vec::new();
    };
    let mut hits = Vec::new();
    for (i, chunk) in blob.chunks_exact(8).enumerate() {
        let candidate = u64::from_le_bytes(chunk.try_into().unwrap());
        if !plausible_ptr(candidate) {
            continue;
        }
        let Some(vt_bytes) = remote.read(candidate, 8) else { continue };
        let obj_vtable = u64::from_le_bytes(vt_bytes.try_into().unwrap());
        if !plausible_ptr(obj_vtable) {
            continue;
        }
        let Some(kb) = remote.read(obj_vtable + mono_layout::MONO_VTABLE_KLASS, 8) else { continue };
        if u64::from_le_bytes(kb.try_into().unwrap()) == class_ptr {
            let off = i as u64 * 8;
            println!(
                "    vtable+0x48+0x{off:x} (абс. 0x{:x}): 0x{candidate:x} -> \
                 vtable 0x{obj_vtable:x} -> klass збігається!",
                wide_start + off
            );
            hits.push(candidate);
        }
    }
    hits
}

/// For every class this session's chat named as a live-tracking target,
/// try to find a static field holding an instance of itself -- the
/// pattern every C# singleton (`Foo.Get()`, `Foo.Instance`, ...) compiles
/// down to. Returns `(class_name, class_ptr, vtable_ptr)` for every class
/// a `MonoVTable*` was found for, whether or not a singleton was, since
/// the vtable pointer itself is exactly what a heap scan for live
/// instances needs next (see `scan_heap_for_class`).
fn find_singletons(remote: &Remote, classes: &[(String, u64)]) -> Vec<(String, u64, u64)> {
    const TARGETS: &[&str] = &["DraftManager", "PowerProcessor", "GameState", "Entity", "Player"];
    println!("\n--deep: пошук статичних синглтонів для {TARGETS:?}");
    let mut resolved = Vec::new();
    for &target in TARGETS {
        let Some(&(_, class_ptr)) = classes.iter().find(|(n, _)| n == target) else {
            println!("  {target}: класу з такою точною назвою немає в дампі");
            continue;
        };
        println!("  {target}: MonoClass* = 0x{class_ptr:x}");
        let Some(vtable) = find_vtable(remote, class_ptr) else {
            println!(
                "    немає vtable (runtime_info порожній або жоден із перших \
                 4 слотів домену не пройшов перевірку klass) -- або клас ще \
                 не використовувався рушієм, або MONO_CLASS_RUNTIME_INFO у \
                 mono_layout.rs (обчислений, не підтверджений) невірний"
            );
            continue;
        };
        println!("    MonoVTable* = 0x{vtable:x}");
        resolved.push((target.to_string(), class_ptr, vtable));
        let hits = find_self_typed_static(remote, class_ptr, vtable);
        if hits.is_empty() {
            println!(
                "    жодного статичного поля з екземпляром цього ж типу в \
                 ширшому діапазоні -- ймовірно, синглтон зберігається інакше \
                 (ServiceLocator, а не static-поле)."
            );
        }
    }
    resolved
}

/// Every live object of `class_name` has, as its first 8 bytes, a pointer
/// to exactly the one `MonoVTable*` `find_vtable` resolved for that class
/// -- so instead of the two-dereference "is this a pointer to a pointer
/// to our class" check used everywhere else in this file (too slow across
/// gigabytes of heap, at one `process_vm_readv` call per candidate), this
/// searches for the literal 8-byte pattern of that one known address
/// directly in each large chunk read from the heap. A match's *address*
/// is the object itself -- no second read needed to confirm it.
///
/// Scans every anonymous (no backing file), writable region from
/// `/proc/PID/maps` -- exactly where a GC'd (Boehm, per `mono-2.0-bdwgc`)
/// heap lives, and also where the process's various other anonymous
/// allocations (thread stacks, Wine's own bookkeeping) live, which this
/// makes no attempt to distinguish from the real Mono heap. Capped so a
/// very large or unusual process doesn't turn this into a minutes-long
/// scan; the cap has room to spare against what a Unity client's Mono
/// heap actually tends to look like.
fn scan_heap_for_class(remote: &Remote, pid: u32, class_name: &str, vtable: u64) -> Vec<u64> {
    println!("\n--deep: сканую купчу пам'яті на живі об'єкти {class_name} (vtable=0x{vtable:x})");
    let Ok(maps) = procfs::read_maps(pid) else {
        eprintln!("не зміг прочитати /proc/{pid}/maps");
        return Vec::new();
    };
    let pattern = vtable.to_le_bytes();
    const CHUNK: u64 = 4 * 1024 * 1024;
    const MAX_SCAN_BYTES: u64 = 3_000_000_000;
    const MAX_HITS: usize = 500;

    let mut scanned: u64 = 0;
    let mut hits: Vec<u64> = Vec::new();
    let regions: Vec<&procfs::MapEntry> = maps
        .iter()
        .filter(|m| m.pathname.is_empty() && m.perms.starts_with("rw"))
        .collect();
    println!("  {} анонімних rw-регіонів у мапі процесу", regions.len());

    'outer: for region in &regions {
        let mut addr = region.start;
        while addr < region.end {
            if scanned >= MAX_SCAN_BYTES || hits.len() >= MAX_HITS {
                break 'outer;
            }
            let len = ((region.end - addr).min(CHUNK)) as usize;
            if let Some(buf) = remote.read(addr, len) {
                scanned += len as u64;
                let mut i = 0;
                // Chunk boundaries can split an 8-byte match; accepted as
                // a rare, harmless miss rather than adding overlap-read
                // complexity for a diagnostic tool.
                while i + 8 <= buf.len() {
                    if buf[i..i + 8] == pattern {
                        hits.push(addr + i as u64);
                        if hits.len() >= MAX_HITS {
                            break;
                        }
                    }
                    i += 8;
                }
            }
            addr += len as u64;
        }
    }

    println!(
        "  проскановано {:.0} МБ, {} збігів{}",
        scanned as f64 / 1_048_576.0,
        hits.len(),
        if hits.len() >= MAX_HITS { " (досягнуто ліміту)" } else { "" }
    );
    for &h in hits.iter().take(20) {
        println!("    {class_name}* кандидат: 0x{h:x}");
    }
    if hits.is_empty() {
        eprintln!(
            "жодного об'єкта не знайдено. Можливо, зараз немає активної гри/\
             екрана з такими об'єктами (спробуйте під час бою чи драфту), або \
             ці анонімні rw-регіони — не той пул пам'яті, де Mono тримає купу \
             (Boehm GC іноді резервує пам'ять нетиповим способом під Wine)."
        );
    } else {
        println!(
            "    ... ще {} (показано перші 20)",
            hits.len().saturating_sub(20)
        );
    }
    hits
}

/// Dump one live object's own field bytes, with no `MonoClassField`
/// knowledge at all -- the same reasoning as everywhere else in this
/// file, just applied to instance data instead of class metadata: a
/// pointer field that happens to hold a card ID string (`"CS2_182"`,
/// `"UNG_999t2"`, ...) will decode through `looks_like_name` exactly the
/// way an assembly or class name did, and a small positive 32-bit
/// integer in a plausible range is worth a human's eye even without
/// knowing which field it is. This is a much weaker signal than the
/// earlier scans (nothing here is structurally validated the way
/// `klass == class_ptr` was), so results are reported as candidates to
/// look at, not as confirmed fields.
fn dump_object_fields(remote: &Remote, addr: u64, class_name: &str) {
    println!("\n--deep: сирий дамп полів {class_name}* = 0x{addr:x}");
    const DUMP_LEN: usize = 0x180;
    let Some(bytes) = remote.read(addr, DUMP_LEN) else {
        eprintln!("не зміг прочитати 0x{DUMP_LEN:x} байтів з 0x{addr:x}");
        return;
    };

    println!("  сирі байти:");
    for (row, chunk) in bytes.chunks(16).enumerate() {
        println!("    +0x{:03x}: {}", row * 16, hex(chunk));
    }

    println!("  рядки, знайдені через вказівники в полях (char* ascii):");
    let mut any_string = false;
    for off in (8..DUMP_LEN - 8).step_by(8) {
        let ptr = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        if !plausible_ptr(ptr) {
            continue;
        }
        let Some(str_bytes) = remote.read(ptr, 64) else { continue };
        if let Some(s) = looks_like_name(&str_bytes) {
            println!("    +0x{off:x}: \"{s}\"");
            any_string = true;
        }
    }
    if !any_string {
        println!("    (жодної)");
    }

    println!("  рядки, знайдені через вказівники в полях (System.String / MonoString, UTF-16):");
    let mut any_mono_string = false;
    for off in (8..DUMP_LEN - 8).step_by(8) {
        let ptr = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        if let Some(s) = try_mono_string(remote, ptr) {
            println!("    +0x{off:x}: \"{s}\"");
            any_mono_string = true;
        }
    }
    if !any_mono_string {
        println!("    (жодної)");
    }

    println!("  малі 32-бітні числа (кандидати на id/тег, діапазон 1..=2000):");
    let mut any_int = false;
    for off in (8..DUMP_LEN - 4).step_by(4) {
        let v = i32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        if (1..=2000).contains(&v) {
            println!("    +0x{off:x}: {v}");
            any_int = true;
        }
    }
    if !any_int {
        println!("    (жодного)");
    }
}

/// The heap scan's own hit addresses are the strongest structural evidence
/// found so far: for `Entity`, consecutive hits sit exactly `0x40` bytes
/// apart (`0x7387b300, 0x7387b340, 0x7387b380, ...`) -- not because fields
/// bleed across a wider dump window, but because that *is* the object's
/// real size, and many are allocated contiguously. Combined with
/// `try_mono_string` finding a real `cardId` (`"HERO_11bpt"`, ...) at every
/// object's own `+0x30`, this reads every hit as its own tight 0x40-byte
/// record: `+0x30` cardId, `+0x38` a candidate id field, instead of
/// dumping 0x180 bytes (which was really 4-6 *different* objects at once
/// and made the earlier "which offset means what" cross-check misleading).
fn dump_entity_table(remote: &Remote, hits: &[u64]) {
    println!(
        "\n--deep: таблиця Entity (cardId @ +0x{:x}, id @ +0x{:x}, підтверджено live) для всіх {} знайдених об'єктів",
        mono_layout::ENTITY_CARD_ID, mono_layout::ENTITY_ID, hits.len()
    );
    let mut rows: Vec<(u64, Option<String>, i32)> = Vec::new();
    for &addr in hits {
        let Some(bytes) = remote.read(addr, mono_layout::ENTITY_INSTANCE_SIZE as usize) else { continue };
        let card_off = mono_layout::ENTITY_CARD_ID as usize;
        let id_off = mono_layout::ENTITY_ID as usize;
        let card_ptr = u64::from_le_bytes(bytes[card_off..card_off + 8].try_into().unwrap());
        let card_id = try_mono_string(remote, card_ptr);
        let id_field = i32::from_le_bytes(bytes[id_off..id_off + 4].try_into().unwrap());
        rows.push((addr, card_id, id_field));
    }
    for (addr, card_id, id_field) in &rows {
        println!(
            "  0x{addr:x}: cardId={:<20} +0x38={id_field}",
            card_id.as_deref().unwrap_or("?")
        );
    }
    if let Some((addr, card_id, id_field)) = rows
        .iter()
        .find(|(_, c, _)| c.as_deref().map(|s| s.starts_with("HERO_09dbp")).unwrap_or(false))
    {
        println!(
            "\n  !!! знайдено Lesser Heal (HERO_09dbp) на 0x{addr:x}: cardId={:?}, +0x38={id_field} \
             -- у Power.log цей ентіті має id=57, player=2. Якщо {id_field}==57, +0x38 це EntityID; \
             якщо {id_field}==2, це Controller (власник).",
            card_id
        );
        inspect_entity_tag_pointers(remote, *addr, "Lesser Heal (HERO_09dbp, id=57, controller=2)");
    } else {
        println!("\n  (Lesser Heal серед знайдених 500 не трапився цього разу -- це не проблема, спробуйте ще раз під час гри, або назвіть мені яку карту зіграно щойно і її id з Power.log, шукатимемо саме її)");
    }
}

/// Same idea as `dump_entity_table`, for `Player` -- `PLAYER_NAME` and
/// `PLAYER_PLAYER_ID` are both confirmed live (see `mono_layout.rs`), but
/// on only 2 samples so far (one game's two players), unlike `ENTITY_ID`'s
/// exact single-value match. Worth another game's worth of confirmation
/// before this is trusted as hard as the Entity table above.
fn dump_player_table(remote: &Remote, hits: &[u64]) {
    println!(
        "\n--deep: таблиця Player (ім'я @ +0x{:x}, playerId @ +0x{:x}, підтверджено лише на 2 зразках) для всіх {} знайдених об'єктів",
        mono_layout::PLAYER_NAME, mono_layout::PLAYER_PLAYER_ID, hits.len()
    );
    const WINDOW: usize = 0x200;
    for &addr in hits {
        let Some(bytes) = remote.read(addr, WINDOW) else { continue };
        let name_off = mono_layout::PLAYER_NAME as usize;
        let id_off = mono_layout::PLAYER_PLAYER_ID as usize;
        let name_ptr = u64::from_le_bytes(bytes[name_off..name_off + 8].try_into().unwrap());
        let name = try_mono_string(remote, name_ptr);
        let player_id = i32::from_le_bytes(bytes[id_off..id_off + 4].try_into().unwrap());
        println!(
            "  0x{addr:x}: ім'я={:<20} playerId={player_id}",
            name.as_deref().unwrap_or("?")
        );
    }
}

/// For one specific Entity, follow each of the 4 still-unidentified pointer
/// fields (`ENTITY_UNKNOWN_PTRS`, see `mono_layout.rs`) and dump a chunk of
/// what each points at, flagging anything that looks like a `Dictionary<K,
/// V>`'s `buckets` array -- Mono/.NET's `Dictionary` stores empty buckets
/// as `-1` (`0xffffffff`), so an int32 array with several of those mixed
/// with small positive numbers is the standard tell for "this is a
/// dictionary's bucket table", which is what `Entity`'s per-tag
/// (`ZONE`/`CONTROLLER`/...) storage is expected to be shaped like.
fn inspect_entity_tag_pointers(remote: &Remote, addr: u64, label: &str) {
    println!("\n--deep: досліджую 4 невідомі вказівники {label} (0x{addr:x}) -- шукаю щось схоже на Dictionary<GameTag,int>");
    let Some(obj) = remote.read(addr, mono_layout::ENTITY_INSTANCE_SIZE as usize) else {
        eprintln!("  не зміг прочитати сам об'єкт");
        return;
    };
    for &off in &mono_layout::ENTITY_UNKNOWN_PTRS {
        let off = off as usize;
        let ptr = u64::from_le_bytes(obj[off..off + 8].try_into().unwrap());
        if !plausible_ptr(ptr) {
            println!("  +0x{off:x}: 0x{ptr:x} -- не схоже на вказівник, пропускаю");
            continue;
        }
        let Some(target) = remote.read(ptr, 0x90) else {
            println!("  +0x{off:x}: 0x{ptr:x} -- не зміг прочитати ціль");
            continue;
        };
        let ints: Vec<i32> = target
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let neg_one_count = ints.iter().filter(|&&v| v == -1).count();
        let small_positive_count = ints.iter().filter(|&&v| (0..64).contains(&v)).count();
        let dict_like = neg_one_count >= 2 && small_positive_count >= 4;
        println!(
            "  +0x{off:x}: 0x{ptr:x}{}",
            if dict_like { "  <-- схоже на Dictionary.buckets (кілька -1 + малі числа)" } else { "" }
        );
        println!("    {}", hex(&target[..0x40]));
        println!("    {}", hex(&target[0x40..0x80]));
    }
}

fn read_cstring(remote: &Remote, addr: u64, max: usize) -> Option<String> {
    if addr == 0 {
        return None;
    }
    let bytes = remote.read(addr, max)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec()).ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}
