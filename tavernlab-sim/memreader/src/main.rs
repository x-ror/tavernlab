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
                dump_class_names(&remote, image);
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
fn dump_class_names(remote: &Remote, image: u64) {
    println!("\n--deep: MonoImage.class_cache -> усі класи Assembly-CSharp (офсети підтверджено)");
    let table_ptr_addr = image + mono_layout::MONO_IMAGE_CLASS_CACHE_TABLE;
    let Some(tp) = remote.read(table_ptr_addr, 8) else {
        eprintln!("не зміг прочитати вказівник таблиці за 0x{table_ptr_addr:x}");
        return;
    };
    let table_ptr = u64::from_le_bytes(tp.try_into().unwrap());
    if !plausible_ptr(table_ptr) {
        eprintln!("0x{table_ptr:x} не схоже на вказівник — офсет міг застаріти");
        return;
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
        return;
    }

    let mut names: Vec<String> = Vec::new();
    let mut interesting: Vec<String> = Vec::new();
    for &bucket in buckets.iter().filter(|&&b| plausible_ptr(b)) {
        let Some(np) = remote.read(bucket + mono_layout::MONO_CLASS_NAME, 8) else { continue };
        let name_ptr = u64::from_le_bytes(np.try_into().unwrap());
        let Some(name) = read_cstring(remote, name_ptr, 96) else { continue };
        if name.is_empty() {
            continue;
        }
        if INTERESTING_SUBSTRINGS.iter().any(|kw| name.contains(kw)) {
            interesting.push(name.clone());
        }
        names.push(name);
    }
    println!(
        "  {} непорожніх бакетів, {} з них дали назву класу",
        buckets.iter().filter(|&&b| plausible_ptr(b)).count(),
        names.len()
    );
    if !interesting.is_empty() {
        interesting.sort();
        interesting.dedup();
        println!("\n  !!! класи, що збігаються з ключовими словами:");
        for n in &interesting {
            println!("    {n}");
        }
    } else {
        println!(
            "\n  жодного класу не збігається з {:?}. Скиньте мені весь \
             вивід — і, якщо можете, назвіть кілька класів з ним, які \
             видались доречними (може, потрібне інше ключове слово, чи \
             клас у іншій асемблі, не Assembly-CSharp).",
            INTERESTING_SUBSTRINGS
        );
    }
    println!(
        "\n  Повний список ({} класів) варто зберегти окремо, якщо хочете \
         його переглянути — тут виводяться лише збіги з ключовими словами. \
         Додайте --scan-classes замість --deep, якщо потрібен діагностичний \
         режим сканування з нуля (повільніший, підтверджує офсети наново).",
        names.len()
    );
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
