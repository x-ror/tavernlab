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

    // --deep: MonoDomain's own layout is unverified (mono_layout.rs), so
    // rather than trust one guessed offset this scans for it — see
    // `scan_for_assembly_list`. `deep_walk_fixed_offset` is what runs once
    // that scan has told us the real offsets.
    scan_for_assembly_list(&remote, domain_addr);
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
/// or class name, not binary data. Deliberately strict: this is what
/// keeps the brute-force scan below from reporting noise.
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

/// `MonoDomain` layout is unverified (see `mono_layout.rs`), so instead of
/// trusting one guessed offset this scans every 8-byte-aligned slot in the
/// first 2 KiB of the domain struct, treats each as a candidate
/// `GList*` (`{data, next}`), and treats *its* `data` as a candidate
/// `MonoAssembly*` by trying a handful of small offsets for a name
/// pointer. A hit that decodes to a real-looking name (`mscorlib`,
/// `Assembly-CSharp`, ...) tells us three things it is otherwise very
/// hard to separately guess right: the domain_assemblies offset, the
/// GList node shape, and where the name pointer sits inside MonoAssembly
/// — all from one piece of positive evidence instead of one blind offset.
fn scan_for_assembly_list(remote: &Remote, domain_addr: u64) {
    println!("\n--deep: MonoDomain layout невідомий, скануюсь замість здогадки");
    let Some(region) = remote.read(domain_addr, 0x800) else {
        eprintln!("не зміг прочитати 0x800 байтів з MonoDomain* — сама адреса, найімовірніше, хибна");
        return;
    };
    let candidate_name_offsets: &[u64] = &[0, 8, 16, 24, 32, 40, 48];
    let mut hits = 0;
    for domain_off in (0..region.len() - 8).step_by(8) {
        let head = u64::from_le_bytes(region[domain_off..domain_off + 8].try_into().unwrap());
        if !plausible_ptr(head) {
            continue;
        }
        let Some(node) = remote.read(head, 16) else { continue };
        let data = u64::from_le_bytes(node[0..8].try_into().unwrap());
        if !plausible_ptr(data) {
            continue;
        }
        for &name_off in candidate_name_offsets {
            let Some(ptr_bytes) = remote.read(data + name_off, 8) else { continue };
            let name_ptr = u64::from_le_bytes(ptr_bytes.try_into().unwrap());
            if !plausible_ptr(name_ptr) {
                continue;
            }
            let Some(str_bytes) = remote.read(name_ptr, 64) else { continue };
            if let Some(name) = looks_like_name(&str_bytes) {
                hits += 1;
                println!(
                    "  кандидат: MonoDomain+0x{domain_off:x} -> GList.data=0x{data:x} \
                     -> +0x{name_off:x} -> \"{name}\""
                );
                if hits > 40 {
                    println!("  (зупиняюсь на 40 кандидатах, щоб не заспамити вивід)");
                    return;
                }
            }
        }
    }
    if hits == 0 {
        eprintln!(
            "жодного кандидата не знайдено в перших 0x800 байтах MonoDomain. \
             Можливо GList-вузол лежить не одразу за вказівником-головою (є \
             ще один рівень непрямості), або назва читається не з перших \
             48 байтів MonoAssembly. Надішліть мені весь вивід — розширю \
             діапазон сканування."
        );
        return;
    }
    println!(
        "\nПодивіться на список вище: рядок \"mscorlib\" або \"Assembly-CSharp\" \
         серед кандидатів — це знахідка. Скиньте мені весь блок \"кандидат: ...\", \
         і я перетворю правильний рядок на постійні офсети в mono_layout.rs."
    );
}

#[allow(dead_code)]
fn deep_walk_fixed_offset(remote: &Remote, domain_addr: u64) {
    use mono_layout::*;

    println!("\n--deep: пробую пройти MonoDomain -> assemblies -> Assembly-CSharp");
    let Some(head_bytes) = remote.read(domain_addr + MONO_DOMAIN_DOMAIN_ASSEMBLIES, 8) else {
        eprintln!("не зміг прочитати domain_assemblies за офсетом 0x{MONO_DOMAIN_DOMAIN_ASSEMBLIES:x}");
        return;
    };
    let mut node = u64::from_le_bytes(head_bytes.try_into().unwrap());
    let mut hops = 0;
    while node != 0 && hops < 64 {
        hops += 1;
        // GList: { data: *MonoAssembly, next: *GList }
        let Some(pair) = remote.read(node, 16) else { break };
        let data = u64::from_le_bytes(pair[0..8].try_into().unwrap());
        let next = u64::from_le_bytes(pair[8..16].try_into().unwrap());
        if data != 0 {
            if let Some(name_ptr_bytes) = remote.read(data + MONO_ASSEMBLY_NAME, 8) {
                let name_ptr = u64::from_le_bytes(name_ptr_bytes.try_into().unwrap());
                if let Some(name) = read_cstring(remote, name_ptr, 64) {
                    println!("  assembly: {name}");
                    if name == "Assembly-CSharp" {
                        if let Some(img_bytes) = remote.read(data + MONO_ASSEMBLY_IMAGE, 8) {
                            let image = u64::from_le_bytes(img_bytes.try_into().unwrap());
                            println!("  -> Assembly-CSharp MonoImage*: 0x{image:x}");
                            println!(
                                "  (наступний крок — пройти image->class_cache; офсет \
                                 MONO_IMAGE_CLASS_CACHE у mono_layout.rs поки не \
                                 перевірений на цьому білді)"
                            );
                        }
                    }
                }
            }
        }
        node = next;
    }
    if hops == 0 {
        eprintln!(
            "жодного вузла у зв'язному списку asssemblies — офсет \
             MONO_DOMAIN_DOMAIN_ASSEMBLIES (0x{MONO_DOMAIN_DOMAIN_ASSEMBLIES:x}) \
             майже напевно неправильний для цього білда. Це очікувано на \
             першому проході; надішліть мені вивід і адресу MonoDomain* \
             вище, і я перевірю офсет іншим шляхом."
        );
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
