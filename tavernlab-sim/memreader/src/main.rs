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
    eprintln!("PID Hearthstone.exe: {pid}");

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
    eprintln!(
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
    eprintln!("PE розпарсено, {} експортів знайдено", exports.len());

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
    eprintln!("mono_get_root_domain: RVA=0x{rva:x} -> адреса в процесі 0x{func_addr:x}");

    let remote = Remote::new(pid);

    // Ad-hoc follow-up on a specific address already found some other way
    // (a `find_self_typed_static`/`scan_all_for_self_typed_statics` hit,
    // typically) -- doesn't need any of the domain/class-cache walk below,
    // just the raw read `dump_object_fields` already knows how to do.
    if mode == "--dump-addr" {
        let Some(hex_arg) = args.get(2).map(|s| s.trim_start_matches("0x")) else {
            eprintln!("використання: memreader --dump-addr 0x<адреса>");
            exit(1);
        };
        let Ok(addr) = u64::from_str_radix(hex_arg, 16) else {
            eprintln!("не зрозумів адресу \"{hex_arg}\" як шістнадцяткове число");
            exit(1);
        };
        dump_object_fields(&remote, addr, hex_arg);
        return;
    }

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
    eprintln!("перші 16 байтів функції: {}", hex(&prologue));

    let Some(domain_ptr_addr) = decode_root_domain_thunk(func_addr, &prologue) else {
        eprintln!(
            "не впізнав опкод у прологу — очікував `48 8b 05 xx xx xx xx c3` \
             (mov rax, [rip+disp32]; ret), можливо з проміжним jmp-thunk. \
             Скиньте мені рядок з перших 16 байтів вище, і я підправлю \
             декодер під цей конкретний білд."
        );
        exit(1);
    };
    eprintln!("адреса глобальної змінної root domain: 0x{domain_ptr_addr:x}");

    let Some(domain_bytes) = remote.read(domain_ptr_addr, 8) else {
        eprintln!("не зміг прочитати 8 байтів з 0x{domain_ptr_addr:x}");
        exit(1);
    };
    let domain_addr = u64::from_le_bytes(domain_bytes.try_into().unwrap());
    eprintln!("MonoDomain*: 0x{domain_addr:x}");

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
        eprintln!(
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
            } else if mode == "--find-all-singletons" {
                let classes = dump_class_names(&remote, image);
                scan_all_for_self_typed_statics(&remote, &classes);
            } else if mode == "--find-refs-to" {
                let targets: Vec<u64> = args
                    .get(2)
                    .map(|s| s.as_str())
                    .unwrap_or("")
                    .split(',')
                    .filter_map(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
                    .collect();
                if targets.is_empty() {
                    eprintln!("використання: memreader --find-refs-to 0x<адреса1>,0x<адреса2>,...");
                    exit(1);
                }
                let classes = dump_class_names(&remote, image);
                scan_all_statics_for_addresses(&remote, &classes, &targets);
            } else {
                let classes = dump_class_names(&remote, image);
                let resolved = find_singletons(&remote, &classes);

                if mode == "--snapshot" || mode == "--board" {
                    print_snapshot(&remote, pid, &resolved, mode == "--board");
                    return;
                }

                for target in ["Entity", "Player"] {
                    if let Some(&(_, _, vtable)) =
                        resolved.iter().find(|(n, _, _)| n == target)
                    {
                        let hits = scan_heap_for_class(&remote, pid, target, vtable, 500);
                        eprintln!(
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
                            eprintln!(
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

/// `--snapshot`: the machine-readable counterpart to `--deep`'s human
/// diagnostics. Every diagnostic print in this file goes to stderr, so
/// stdout carries *only* this one JSON document -- safe for a caller
/// (`tavernsim watch`) to pipe and parse directly, unlike `--deep`'s prose
/// output. All offsets used here (`ENTITY_CARD_ID`/`ENTITY_ID`/
/// `ENTITY_TAG_LIST`/`PLAYER_NAME`/`PLAYER_PLAYER_ID`) are the ones
/// `mono_layout.rs` marks confirmed live -- see `README.md` for the
/// evidence behind each.
fn print_snapshot(remote: &Remote, pid: u32, resolved: &[(String, u64, u64)], board: bool) {
    use tavernlab_json::Out;

    let mut entities: Vec<EntityHit> = Vec::new();
    let mut players: Vec<PlayerHit> = Vec::new();
    if let Some(&(_, _, vtable)) = resolved.iter().find(|(n, _, _)| n == "Entity") {
        let hits = scan_heap_for_class(remote, pid, "Entity", vtable, 12_000);
        entities = collect_entity_hits(remote, &hits);
    }
    if let Some(&(_, _, vtable)) = resolved.iter().find(|(n, _, _)| n == "Player") {
        let hits = scan_heap_for_class(remote, pid, "Player", vtable, 200);
        players = collect_player_hits(remote, &hits);
    }

    let live = current_game_entities(&entities);
    let game = read_game_state(&live);
    let sides = build_sides(&live, &players, game.current_player);

    if board {
        print_board(&game, &sides, &live);
        return;
    }

    let confidence = topology_confidence(&live);

    let mut out = Out::new();
    out.obj(|o| {
        o.field("game", |v| {
            v.obj(|o| {
                o.field("turn", |v| v.opt(game.turn, |o, n| o.int(n as i64)));
                o.field("step", |v| v.opt(game.step, |o, n| o.int(n as i64)));
                o.field("currentPlayer", |v| {
                    v.opt(game.current_player, |o, n| o.int(n as i64))
                });
                o.int_field("idCap", game.id_cap as i64);
            })
        });
        o.str_field("confidence", confidence);
        o.field("entities", |v| {
            v.arr(|a| {
                for e in &live {
                    a.item(|v| emit_entity(v, e));
                }
            })
        });
        o.field("players", |v| {
            v.arr(|a| {
                for p in &players {
                    a.item(|v| {
                        v.obj(|o| {
                            o.field("addr", |v| v.str(&format!("0x{:x}", p.addr)));
                            o.field("name", |v| v.opt(p.name.as_deref(), |o, s| o.str(s)));
                            o.int_field("playerId", p.player_id as i64);
                            o.field("entityId", |v| v.opt(p.entity_id, |o, n| o.int(n as i64)));
                            o.field("rawTags", |v| emit_tags(v, &p.tags));
                        })
                    });
                }
            })
        });
        o.field("sides", |v| {
            v.arr(|a| {
                for s in &sides {
                    a.item(|v| emit_side(v, s));
                }
            })
        });
    });
    println!("{}", out.finish());
}

fn print_board(game: &GameState, sides: &[Side], live: &[EntityHit]) {
    eprintln!(
        "хід {}  крок {}  ходить гравець {}  idCap {}  довіра {}",
        game.turn.map(|n| n.to_string()).unwrap_or("?".into()),
        game.step.map(|n| n.to_string()).unwrap_or("?".into()),
        game.current_player.map(|n| n.to_string()).unwrap_or("?".into()),
        game.id_cap,
        topology_confidence(live)
    );
    for id in [1, 2, 3] {
        if let Some(e) = live.iter().find(|e| e.id == id) {
            eprintln!(
                "  id={id} @{:x} {} tags: {}",
                e.addr,
                e.card_id.as_deref().unwrap_or("-"),
                format_tags(&e.tags)
            );
        }
    }
    for s in sides {
        let star = if game.current_player == Some(s.player_id) {
            "*"
        } else {
            " "
        };
        eprintln!(
            "\n{star} P{} {}  mana {}/{}  armor {}",
            s.player_id,
            s.name.as_deref().unwrap_or("?"),
            s.mana.map(|n| n.to_string()).unwrap_or("?".into()),
            s.mana_max,
            s.armor
        );
        if let Some(h) = &s.hero {
            eprintln!(
                "  hero {} {}/{}",
                h.card_id.as_deref().unwrap_or("?"),
                h.tag(TAG_ATK).unwrap_or(0),
                h.tag(TAG_HEALTH).unwrap_or(0)
            );
        }
        if let Some(hp) = &s.hero_power {
            eprintln!("  power {}", hp.card_id.as_deref().unwrap_or("?"));
        }
        if let Some(w) = &s.weapon {
            eprintln!(
                "  wep {} {}/{}",
                w.card_id.as_deref().unwrap_or("?"),
                w.tag(TAG_ATK).unwrap_or(0),
                w.tag(TAG_HEALTH).unwrap_or(0)
            );
        }
        eprint!("  play:");
        if s.play.is_empty() {
            eprintln!(" (empty)");
        } else {
            eprintln!();
            for e in &s.play {
                eprintln!(
                    "    [{}] {} {}/{}",
                    e.tag(TAG_ZONE_POSITION).unwrap_or(0),
                    e.card_id.as_deref().unwrap_or("?"),
                    e.tag(TAG_ATK).unwrap_or(0),
                    e.tag(TAG_HEALTH).unwrap_or(0)
                );
            }
        }
        eprint!("  hand:");
        if s.hand.is_empty() {
            eprintln!(" (empty)");
        } else {
            eprintln!(
                " {}",
                s.hand
                    .iter()
                    .map(|e| format!(
                        "{}({})",
                        e.card_id.as_deref().unwrap_or("?"),
                        e.tag(TAG_COST).unwrap_or(0)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !s.secret.is_empty() {
            eprintln!(
                "  secret: {}",
                s.secret
                    .iter()
                    .map(|e| e.card_id.as_deref().unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        eprintln!("  deck {}  gy {}", s.deck, s.graveyard);
    }
}

#[derive(Clone)]
struct EntityHit {
    addr: u64,
    card_id: Option<String>,
    id: i32,
    tags: Vec<(i32, i32)>,
}

struct PlayerHit {
    addr: u64,
    name: Option<String>,
    player_id: i32,
    /// Always `None` today. `mono_layout::ENTITY_ID` (+0x38) is confirmed
    /// live on `Entity` only -- `Player.name` sits at a completely
    /// different offset (+0x120) than `Entity.cardId` (+0x30), so there is
    /// no reason to expect the two classes share a layout past their
    /// common `MonoObject` header, and a live dump confirmed it: reading
    /// +0x38 off a real, correctly-matched `Player` gave ~1e9 (nonsense
    /// for an EntityID), and off a stale, wrong-match `Player` gave a
    /// negative garbage word. Neither is usable, so `collect_player_hits`
    /// no longer reads it. The obvious alternative -- GameTag 53
    /// (`ENTITY_ID`) from this object's own `List<Tag>`, the way `Entity`'s
    /// tags already are -- isn't available either yet: `read_entity_tags`'s
    /// `List<Tag>` offset was confirmed live against `Entity` objects only,
    /// and pointed at a live `Player` address it read back *empty* (i.e.
    /// wherever `Player`'s own tag list actually lives, it isn't at that
    /// offset). Do not restore the +0x38 read as a "fix" -- it was never
    /// reading the right field, on either class. Leave this `None` until a
    /// real `Player` tag-list or `ENTITY_ID` offset is confirmed live the
    /// same way `PLAYER_NAME`/`PLAYER_PLAYER_ID` were.
    entity_id: Option<i32>,
    tags: Vec<(i32, i32)>,
}

struct GameState {
    turn: Option<i32>,
    step: Option<i32>,
    current_player: Option<i32>,
    id_cap: i32,
}

impl EntityHit {
    fn tag(&self, name: i32) -> Option<i32> {
        find_tag(&self.tags, name)
    }
}

/// GameTag values used in the snapshot. Numbers are the enum the client
/// writes to Power.log (`tag=ZONE value=PLAY` etc.).
const TAG_STEP: i32 = 19;
const TAG_TURN: i32 = 20;
const TAG_CURRENT_PLAYER: i32 = 23;
const TAG_RESOURCES_USED: i32 = 25;
const TAG_RESOURCES: i32 = 26;
const TAG_HERO_ENTITY: i32 = 27;
const _TAG_MAXHANDSIZE: i32 = 28;
const TAG_PLAYER_ID: i32 = 30;
const TAG_HEALTH: i32 = 45;
const TAG_ATK: i32 = 47;
const TAG_COST: i32 = 48;
const TAG_ZONE: i32 = 49;
const TAG_CONTROLLER: i32 = 50;
const TAG_ARMOR: i32 = 292;
const TAG_CARDTYPE: i32 = 202;
const TAG_ZONE_POSITION: i32 = 263;
const TAG_TEMP_RESOURCES: i32 = 295;

const ZONE_PLAY: i32 = 1;
const ZONE_DECK: i32 = 2;
const ZONE_HAND: i32 = 3;
const ZONE_GRAVEYARD: i32 = 4;
const ZONE_SECRET: i32 = 7;

const CARDTYPE_HERO: i32 = 3;
const CARDTYPE_MINION: i32 = 4;
const CARDTYPE_WEAPON: i32 = 7;
const CARDTYPE_HERO_POWER: i32 = 10;

fn emit_entity(v: &mut tavernlab_json::Out, e: &EntityHit) {
    v.obj(|o| {
        o.field("addr", |v| v.str(&format!("0x{:x}", e.addr)));
        o.field("cardId", |v| v.opt(e.card_id.as_deref(), |o, s| o.str(s)));
        o.int_field("id", e.id as i64);
        o.field("zone", |v| v.opt(e.tag(TAG_ZONE), |o, n| o.int(n as i64)));
        o.field("controller", |v| {
            v.opt(e.tag(TAG_CONTROLLER), |o, n| o.int(n as i64))
        });
        o.field("cardType", |v| {
            v.opt(e.tag(TAG_CARDTYPE), |o, n| o.int(n as i64))
        });
        o.field("atk", |v| v.opt(e.tag(TAG_ATK), |o, n| o.int(n as i64)));
        o.field("health", |v| v.opt(e.tag(TAG_HEALTH), |o, n| o.int(n as i64)));
        o.field("cost", |v| v.opt(e.tag(TAG_COST), |o, n| o.int(n as i64)));
        o.field("zonePosition", |v| {
            v.opt(e.tag(TAG_ZONE_POSITION), |o, n| o.int(n as i64))
        });
        o.field("rawTags", |v| emit_tags(v, &e.tags));
    });
}

/// Every `(GameTag, value)` pair this object's own `List<Tag>` carried, not
/// just the handful of curated fields above -- so the next live session
/// can cross-check a real, known board state (Taunt just granted, damage
/// just taken, ...) against whatever numeric tag changed, straight through
/// `/api/memory`, without needing a terminal session running `memreader`
/// by hand the way every earlier round of this reverse-engineering did.
/// `name` is only ever the small set `game_tag_name` already recognises
/// (confirmed live earlier this project) -- an unrecognised `tag` number is
/// not a guess at what it means, just the number itself.
fn emit_tags(v: &mut tavernlab_json::Out, tags: &[(i32, i32)]) {
    v.arr(|a| {
        for &(name, value) in tags {
            a.item(|v| {
                v.obj(|o| {
                    o.int_field("tag", name as i64);
                    o.int_field("value", value as i64);
                    if let Some(known) = game_tag_name(name) {
                        o.str_field("name", known);
                    }
                });
            });
        }
    });
}

struct Side {
    player_id: i32,
    name: Option<String>,
    current: bool,
    mana: Option<i32>,
    mana_max: i32,
    armor: i32,
    hero: Option<EntityHit>,
    hero_power: Option<EntityHit>,
    weapon: Option<EntityHit>,
    play: Vec<EntityHit>,
    hand: Vec<EntityHit>,
    secret: Vec<EntityHit>,
    deck: usize,
    graveyard: usize,
}

fn emit_side(v: &mut tavernlab_json::Out, s: &Side) {
    v.obj(|o| {
        o.int_field("playerId", s.player_id as i64);
        o.field("name", |v| v.opt(s.name.as_deref(), |o, n| o.str(n)));
        o.bool_field("current", s.current);
        o.field("mana", |v| v.opt(s.mana, |o, n| o.int(n as i64)));
        o.int_field("manaMax", s.mana_max as i64);
        o.int_field("armor", s.armor as i64);
        o.field("hero", |v| match &s.hero {
            Some(e) => emit_entity(v, e),
            None => v.null(),
        });
        o.field("heroPower", |v| match &s.hero_power {
            Some(e) => emit_entity(v, e),
            None => v.null(),
        });
        o.field("weapon", |v| match &s.weapon {
            Some(e) => emit_entity(v, e),
            None => v.null(),
        });
        o.field("play", |v| {
            v.arr(|a| {
                for e in &s.play {
                    a.item(|v| emit_entity(v, e));
                }
            })
        });
        o.field("hand", |v| {
            v.arr(|a| {
                for e in &s.hand {
                    a.item(|v| emit_entity(v, e));
                }
            })
        });
        o.field("secret", |v| {
            v.arr(|a| {
                for e in &s.secret {
                    a.item(|v| emit_entity(v, e));
                }
            })
        });
        o.int_field("deck", s.deck as i64);
        o.int_field("graveyard", s.graveyard as i64);
    });
}

/// One copy per EntityID. Prefer the object closest to `anchor` (the live
/// GameEntity): leftovers from earlier matches keep the same ids but sit
/// in older heap islands.
fn dedup_by_id(hits: &[EntityHit], anchor: u64) -> Vec<EntityHit> {
    let mut best: Vec<EntityHit> = Vec::new();
    for e in hits {
        if e.id <= 0 {
            continue;
        }
        if let Some(existing) = best.iter_mut().find(|x| x.id == e.id) {
            let rank = |x: &EntityHit| {
                let filled = x.card_id.is_some() as i32 + x.tag(TAG_ZONE).is_some() as i32;
                (filled, std::cmp::Reverse(x.addr.abs_diff(anchor)))
            };
            if rank(e) > rank(existing) {
                *existing = e.clone();
            }
        } else {
            best.push(e.clone());
        }
    }
    best
}

/// How close, in heap address space, an object has to sit to `GameEntity`
/// to count as belonging to the same match. Shared between
/// `current_game_entities` (which entities are "this game") and
/// `build_sides` (which `Player` object is "this game"'s, not a same-name
/// leftover from a stale island the raw nearest-address match would
/// otherwise pick, per the mixed-up-sides report this fixes).
const SAME_GAME_WINDOW: u64 = 32 * 1024 * 1024;

/// The live GameEntity (`id=1`, `CARDTYPE=GAME`) sits in the same heap
/// island as that match's other entities. Leftovers from earlier games
/// keep high ids but live far away. Cap is the highest id in a 32 MiB
/// window around GameEntity.
fn current_game_entities(hits: &[EntityHit]) -> Vec<EntityHit> {
    let Some(game) = game_entity(hits) else {
        return dedup_by_id(hits, 0);
    };
    let deduped = dedup_by_id(hits, game.addr);
    let mut cap = deduped
        .iter()
        .filter(|e| e.addr.abs_diff(game.addr) < SAME_GAME_WINDOW)
        .map(|e| e.id)
        .max()
        .unwrap_or(game.id);
    for e in deduped.iter().filter(|e| e.id == 2 || e.id == 3) {
        if let Some(hid) = e.tag(TAG_HERO_ENTITY) {
            cap = cap.max(hid + 2); // hero power sits on the next id
        }
    }
    deduped.into_iter().filter(|e| e.id <= cap).collect()
}

/// Per-candidate topology score for `game_entity`: `(topology, tag_bonus,
/// neighbour_count)`, compared lexicographically by `max_by_key` (Rust
/// tuples are `Ord` in field order) so later fields can only ever break a
/// *tie* in an earlier one -- they can never outvote it.
///
/// `topology` (real board-legality facts, report A.6 item 2: a hero in
/// PLAY for each controller) is the primary key and the only one that can
/// disqualify a candidate outright (`i32::MIN`, more than 7 PLAY minions
/// for one controller). It was not always the primary key: an earlier cut
/// of this function *added* a flat +5/+5 TURN/STEP bonus straight into the
/// same score as the ±4-per-controller hero signal, so a stale, finished
/// match's leftover TURN/STEP tags (Boehm/bdwgc does not collect a
/// finished match's objects, so they persist with their last known tag
/// values) could add up to more than a live, topologically-equal-or-better
/// candidate that simply had not been tagged -- reproducing the exact
/// swapped-sides bug this rewrite exists to fix, just via TURN/STEP
/// instead of the original raw-neighbour-count heuristic. Lexicographic
/// comparison makes that structurally impossible: `topology` alone decides
/// whenever it differs at all.
///
/// `tag_bonus` (TURN/STEP presence) only matters when `topology` is tied.
/// Kept small and merely present/absent (not weighted 5/5 any more)
/// because, per the point above, presence is *not* reliable evidence of
/// liveness by itself -- a live dump found a real case where the correct,
/// live-Player-holding `id==1` candidate carried neither tag. It is kept
/// as a tie-break, not dropped outright, because with no distinguishing
/// topology at all (neither candidate shows any hero) it is still the
/// best signal available *in this file* -- true liveness proof needs the
/// singleton-walk root (report A.6 item 1 / A.7 PR3) or a temporal lock
/// across polls (PR4), neither of which exists yet. Note this does still
/// leave one case genuinely unresolved by topology alone: two candidates
/// with *identical* legal topology (e.g. both mulligan-fresh, heroes only,
/// no minions -- indistinguishable from "one finished with the loser's
/// hero still standing at buffer-time and the other genuinely live")
/// where only one carries a tag. That is an inherent limit of a
/// topology-only heuristic, not a weighting bug; PR3/PR4 are what actually
/// close it, not further tuning here.
///
/// `neighbour_count` (nearby `id`-2/3 entities) is the last-resort
/// tie-break, below both of the above, restoring the original pre-PR2
/// heuristic (report A.2 point 2) to its proper place as a final
/// determinism device rather than the accidental "last element in scan
/// order wins" that `Iterator::max_by_key` silently falls back to when
/// nothing distinguishes two candidates at all. It carries the same
/// "prefers a fuller stale board" risk the original heuristic had, so it
/// only ever fires once `topology` and `tag_bonus` have both already
/// tied -- i.e. when there is no more-trustworthy signal left to use.
fn game_entity_score(candidate: &EntityHit, hits: &[EntityHit]) -> (i32, i32, usize) {
    let nearby: Vec<&EntityHit> = hits
        .iter()
        .filter(|e| e.addr.abs_diff(candidate.addr) < SAME_GAME_WINDOW)
        .collect();

    let mut topology = 0i32;
    for controller in [1, 2] {
        let has_hero = nearby.iter().any(|e| {
            e.tag(TAG_CONTROLLER) == Some(controller)
                && e.tag(TAG_CARDTYPE) == Some(CARDTYPE_HERO)
                && e.tag(TAG_ZONE) == Some(ZONE_PLAY)
        });
        topology += if has_hero { 4 } else { -4 };

        let minions = nearby
            .iter()
            .filter(|e| {
                e.tag(TAG_CONTROLLER) == Some(controller)
                    && e.tag(TAG_CARDTYPE) == Some(CARDTYPE_MINION)
                    && e.tag(TAG_ZONE) == Some(ZONE_PLAY)
            })
            .count();
        if minions > 7 {
            // Never a legal board -- disqualified outright, not just
            // docked, and the disqualification cannot be bought back by
            // tag_bonus or neighbour_count since it sorts below every
            // non-disqualified topology value.
            return (i32::MIN, i32::MIN, 0);
        }
    }

    let mut tag_bonus = 0i32;
    if candidate.tag(TAG_TURN).is_some() {
        tag_bonus += 1;
    }
    if candidate.tag(TAG_STEP).is_some() {
        tag_bonus += 1;
    }

    let neighbour_count = nearby.iter().filter(|e| e.id == 2 || e.id == 3).count();

    (topology, tag_bonus, neighbour_count)
}

fn game_entity(hits: &[EntityHit]) -> Option<&EntityHit> {
    let candidates: Vec<&EntityHit> = hits.iter().filter(|e| e.id == 1).collect();
    if candidates.len() <= 1 {
        // Nothing to score against -- return the only candidate (or none)
        // exactly as before scoring existed.
        return candidates.into_iter().next();
    }
    // A leftover GameEntity from a finished match is still `id == 1`, can
    // still carry a stale TURN/STEP tag pair from before its match ended,
    // and can easily sit near *more* other entities than a game that just
    // started (a fuller board from a match that ran its course, against a
    // mulligan-fresh one) -- ranking by nearby-player-count alone picked
    // exactly that stale island over the real live game, reported live:
    // `/api/memory` showed a finished match's board while the log watcher
    // correctly tracked a brand-new mulligan. `game_entity_score` scores
    // real board-legality topology instead of raw proximity or presence of
    // a single tag pair.
    candidates
        .into_iter()
        .max_by_key(|g| game_entity_score(g, hits))
}

/// Coarse, honest confidence label for the board `game_entity` picked --
/// report A.7 PR1's "add confidence... when topology fails", the half of
/// that line item that doesn't need a live-confirmed offset to build (see
/// `PlayerHit::entity_id`'s doc comment for the half that does). Not the
/// raw `game_entity_score` tuple itself: its weights are this file's own
/// tuning knobs, not a calibrated probability, so handing them out
/// directly would claim more precision than the heuristic has. `sides`
/// and `entities` still populate unconditionally either way -- the
/// report phrases this as confidence *or* skip, and skipping is the wrong
/// half to take here: `cli`'s `/api/memory` test
/// (`the_memory_endpoint_answers_even_with_no_game_running`) already
/// requires `sides` to always be present, and the board fields (`play`/
/// `hand`/...) are resolved from CONTROLLER/ZONE independently of this
/// score, so they are not wrong just because this label is "low". This is
/// only a hint to a caller about whether to trust the *identity* of the
/// island it got back.
fn topology_confidence(live: &[EntityHit]) -> &'static str {
    match game_entity(live) {
        None => "none",
        Some(g) => match game_entity_score(g, live) {
            (i32::MIN, ..) => "none", // shouldn't happen -- a disqualified island shouldn't have become `live`'s anchor -- but never claim confidence in one if it does
            (topology, ..) if topology >= 8 => "high", // both controllers' hero confirmed in PLAY nearby
            _ => "low", // an id==1 candidate exists, but at least one side's hero was not found nearby
        },
    }
}

fn read_game_state(live: &[EntityHit]) -> GameState {
    let cap = live.iter().map(|e| e.id).max().unwrap_or(0);
    let game = game_entity(live);
    let current_player = game
        .and_then(|e| e.tag(TAG_CURRENT_PLAYER))
        .or_else(|| {
            live.iter().find_map(|e| {
                (e.tag(TAG_CURRENT_PLAYER) == Some(1))
                    .then(|| e.tag(TAG_PLAYER_ID).or_else(|| e.tag(TAG_CONTROLLER)))
                    .flatten()
            })
        });
    GameState {
        turn: game.and_then(|e| e.tag(TAG_TURN)).or_else(|| {
            live.iter().find_map(|e| e.tag(TAG_TURN))
        }),
        step: game.and_then(|e| e.tag(TAG_STEP)).or_else(|| {
            live.iter().find_map(|e| e.tag(TAG_STEP))
        }),
        current_player,
        id_cap: cap,
    }
}

fn build_sides(live: &[EntityHit], players: &[PlayerHit], current_player: Option<i32>) -> Vec<Side> {
    let mut sides = Vec::new();
    for pid in [1, 2] {
        // Current match's Player entities are EntityID 2 (P1) and 3 (P2).
        let want_eid = pid + 1;
        let player_ent = live.iter().find(|e| e.id == want_eid);
        let anchor = game_entity(live).map(|g| g.addr).unwrap_or(0);
        // Prefer a Player object in the same heap island as this match's
        // GameEntity: reported live, the plain "nearest of all ~200 found"
        // match picked a same-`player_id` Player object from a completely
        // different, stale island (a real past opponent's name attached to
        // the live game, hand cards swapped to the wrong side) whenever
        // that stale one happened to sit closer in raw address terms than
        // the actual current one. Deliberately no nearest-overall fallback
        // when nothing for this `pid` is in the window: that fallback *was*
        // the bug -- it is exactly how a stranger's battletag/hand from a
        // long-finished match got attached to a live game (same mechanism
        // this window exists to reject, just applied without the window's
        // protection). Better to leave `name` as `None` (nullable in the
        // JSON) than to hand back a name we have no reason to believe
        // belongs to this match. The board itself (`of`, below) is
        // resolved from `CONTROLLER`/`ZONE` independently of this lookup,
        // so it is unaffected by a missing name.
        let player = players
            .iter()
            .filter(|p| p.player_id == pid && p.name.is_some())
            .filter(|p| p.addr.abs_diff(anchor) < SAME_GAME_WINDOW)
            .min_by_key(|p| p.addr.abs_diff(anchor));
        let name = player.and_then(|p| p.name.clone());
        let of: Vec<&EntityHit> = live
            .iter()
            .filter(|e| e.tag(TAG_CONTROLLER) == Some(pid))
            .collect();
        if of.is_empty() && name.is_none() {
            continue;
        }
        let take = |zone: i32, ty: Option<i32>, cap: usize| -> Vec<EntityHit> {
            let mut v: Vec<EntityHit> = of
                .iter()
                .filter(|e| {
                    e.tag(TAG_ZONE) == Some(zone)
                        && ty.map(|t| e.tag(TAG_CARDTYPE) == Some(t)).unwrap_or(true)
                        && (zone == ZONE_DECK
                            || zone == ZONE_GRAVEYARD
                            || ty == Some(CARDTYPE_HERO)
                            || ty == Some(CARDTYPE_HERO_POWER)
                            || e.card_id.is_some())
                })
                .map(|e| (*e).clone())
                .collect();
            v.sort_by_key(|e| e.tag(TAG_ZONE_POSITION).unwrap_or(0));
            v.truncate(cap);
            v
        };
        let hero = player_ent
            .and_then(|p| p.tag(TAG_HERO_ENTITY))
            .and_then(|hid| live.iter().find(|e| e.id == hid).cloned())
            ;
        let hero_power = hero.as_ref().and_then(|h| {
            live.iter()
                .find(|e| e.id == h.id + 1 && e.tag(TAG_CARDTYPE) == Some(CARDTYPE_HERO_POWER))
                .or_else(|| {
                    live.iter().find(|e| {
                        e.tag(TAG_CARDTYPE) == Some(CARDTYPE_HERO_POWER)
                            && e.tag(TAG_CONTROLLER) == Some(pid)
                            && e.tag(TAG_ZONE) == Some(ZONE_PLAY)
                            && e.card_id.as_deref().is_some_and(|s| s.contains("bp") || s.starts_with("HERO_"))
                    })
                })
                .cloned()
        });
        let armor = hero
            .as_ref()
            .and_then(|h| h.tag(TAG_ARMOR))
            .or_else(|| player.and_then(|p| find_tag(&p.tags, TAG_ARMOR)))
            .unwrap_or(0);
        let mana_src: Vec<(i32, i32)> = {
            let mut t = player_ent.map(|e| e.tags.clone()).unwrap_or_default();
            if let Some(p) = player {
                for pair in &p.tags {
                    if !t.iter().any(|(n, _)| *n == pair.0) {
                        t.push(*pair);
                    }
                }
            }
            t
        };
        let mana_max = find_tag(&mana_src, TAG_RESOURCES)
            .or_else(|| find_tag(&mana_src, 176))
            .unwrap_or(0);
        let mana = find_tag(&mana_src, TAG_RESOURCES).map(|r| {
            let used = find_tag(&mana_src, TAG_RESOURCES_USED).unwrap_or(0);
            let temp = find_tag(&mana_src, TAG_TEMP_RESOURCES).unwrap_or(0);
            (r - used + temp).max(0)
        });
        sides.push(Side {
            player_id: pid,
            name,
            current: current_player == Some(pid),
            mana,
            mana_max,
            armor,
            hero,
            hero_power,
            weapon: take(ZONE_PLAY, Some(CARDTYPE_WEAPON), 1).into_iter().next(),
            play: take(ZONE_PLAY, Some(CARDTYPE_MINION), 7),
            hand: take(ZONE_HAND, None, 10),
            secret: take(ZONE_SECRET, None, 5),
            deck: take(ZONE_DECK, None, 60).len(),
            graveyard: take(ZONE_GRAVEYARD, None, 60).len(),
        });
    }
    sides
}

fn find_tag(tags: &[(i32, i32)], name: i32) -> Option<i32> {
    tags.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// Silent counterpart to `dump_dotnet_list` for `ENTITY_TAG_LIST` only:
/// same `List<Tag>` walk (`_items`/`_size` at `+0x10`/`+0x18`, `MonoArray`
/// vector at `items+0x20`, `Tag.Name`/`Tag.Value` at `+0x10`/`+0x14`), but
/// collects every `(name, value)` pair instead of printing a sample.
fn read_entity_tags(remote: &Remote, entity_addr: u64) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let Some(obj) = remote.read(entity_addr, mono_layout::ENTITY_INSTANCE_SIZE as usize) else {
        return out;
    };
    // Minion samples only filled +0x10, but GameEntity/Player keep TURN /
    // RESOURCES on one of the other three `List<Tag>` slots.
    for &off in &mono_layout::ENTITY_UNKNOWN_PTRS {
        let off = off as usize;
        let list = u64::from_le_bytes(obj[off..off + 8].try_into().unwrap());
        if !plausible_ptr(list) {
            continue;
        }
        for pair in read_tag_list(remote, list) {
            if !out.iter().any(|(n, _)| *n == pair.0) {
                out.push(pair);
            }
        }
    }
    out
}

fn read_tag_list(remote: &Remote, list: u64) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let Some(hdr) = remote.read(list, 0x20) else {
        return out;
    };
    let items = u64::from_le_bytes(hdr[0x10..0x18].try_into().unwrap());
    let size = i32::from_le_bytes(hdr[0x18..0x1c].try_into().unwrap());
    if !(1..=512).contains(&size) || !plausible_ptr(items) {
        return out;
    }
    let n = size as usize;
    let Some(vecb) = remote.read(items + 0x20, n * 8) else {
        return out;
    };
    for i in 0..n {
        let elem = u64::from_le_bytes(vecb[i * 8..i * 8 + 8].try_into().unwrap());
        if !plausible_ptr(elem) {
            continue;
        }
        let Some(tb) = remote.read(elem, 0x18) else { continue };
        let name = i32::from_le_bytes(
            tb[mono_layout::TAG_NAME as usize..mono_layout::TAG_NAME as usize + 4]
                .try_into()
                .unwrap(),
        );
        let value = i32::from_le_bytes(
            tb[mono_layout::TAG_VALUE as usize..mono_layout::TAG_VALUE as usize + 4]
                .try_into()
                .unwrap(),
        );
        out.push((name, value));
    }
    out
}

fn format_tags(tags: &[(i32, i32)]) -> String {
    tags.iter()
        .map(|(k, v)| match game_tag_name(*k) {
            Some(n) => format!("{n}={v}"),
            None => format!("{k}={v}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    // length UTF-16 units plus the trailing 0 the layout promises.
    let char_bytes = remote.read(ptr + 0x14, (len as usize + 1) * 2)?;
    let units: Vec<u16> = char_bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
        .collect();
    if units.get(len as usize).copied() != Some(0) {
        return None;
    }
    if units[..len as usize].iter().any(|&u| u == 0) {
        return None;
    }
    let s = String::from_utf16(&units[..len as usize]).ok()?;
    s.chars()
        .all(|c| c.is_ascii_graphic() || c == ' ')
        .then(|| s)
}

/// Real card ids routinely carry a lowercase suffix -- `HERO_09dbp` (the
/// Priest hero power this file's own `dump_entity_table` uses as a known-
/// good landmark), `EDR_463b`/`EDR_463a`, `JAIL_430e1`, `CATA_476t` all
/// appeared, and were confirmed against a real `Power.log`, earlier in this
/// project. An uppercase-only filter here silently drops exactly that
/// shape -- hero powers and tokens in particular, which is why they were
/// showing up with `cardId: null` in `--snapshot`/`--board` despite being
/// found and tagged correctly otherwise.
fn looks_like_card_id(s: &str) -> bool {
    (4..=32).contains(&s.len())
        && s.contains('_')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `obj -> vtable -> klass -> name` — the same klass check `find_vtable`
/// already uses, just printed so a field pointer can be identified as
/// `TagMap` / `Map` / `Zone` without knowing the offset in advance.
fn class_name_of(remote: &Remote, obj: u64) -> Option<String> {
    if !plausible_ptr(obj) {
        return None;
    }
    let vt = u64::from_le_bytes(remote.read(obj, 8)?.try_into().unwrap());
    if !plausible_ptr(vt) {
        return None;
    }
    let klass = u64::from_le_bytes(
        remote
            .read(vt + mono_layout::MONO_VTABLE_KLASS, 8)?
            .try_into()
            .unwrap(),
    );
    if !plausible_ptr(klass) {
        return None;
    }
    let name_ptr = u64::from_le_bytes(
        remote
            .read(klass + mono_layout::MONO_CLASS_NAME, 8)?
            .try_into()
            .unwrap(),
    );
    read_cstring(remote, name_ptr, 64)
}

/// Walk the confirmed `domain_assemblies` `GList` (see `mono_layout.rs`)
/// and print every assembly name found. Where the scan that discovered the
/// offset had one piece of evidence (`"mscorlib"`), this has dozens —
/// every assembly Hearthstone loads — which is what makes this a
/// confirmation pass rather than another guess.
fn walk_assembly_list(remote: &Remote, domain_addr: u64) -> Option<u64> {
    eprintln!("\n--deep: MonoDomain -> domain_assemblies (офсет підтверджено скануванням)");
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
                    eprintln!("  assembly: {name}");
                    if name == "Assembly-CSharp" {
                        csharp_data = Some(data);
                    }
                }
            }
        }
        node = next;
    }
    eprintln!("({hops} вузлів пройдено)");
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
    eprintln!("\n--deep: MonoAssembly(Assembly-CSharp) -> image (офсет підтверджено)");
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
    eprintln!(
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
    eprintln!("\n--deep: MonoImage.class_cache (сканую — найширший і найповільніший крок)");
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
            eprintln!("  (зупиняюсь на {MAX_TABLES_PROBED} таблицях-кандидатах)");
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
        eprintln!(
            "  таблиця-кандидат: MonoImage+0x{table_off:x} -> 0x{table_ptr:x} \
             ({density}/{BUCKETS_TO_PROBE} заповнено, {}/{total_hits} узгоджено на +0x{dom_off:x})",
            dom_names.len()
        );
        let interesting: Vec<&String> = dom_names
            .iter()
            .filter(|n| INTERESTING_SUBSTRINGS.iter().any(|kw| n.contains(kw)))
            .collect();
        if !interesting.is_empty() {
            eprintln!("    !!! знайдено цікаві назви:");
            for n in &interesting {
                eprintln!("      \"{n}\"");
            }
        }
        eprintln!("    приклади ({} узгоджених):", dom_names.len());
        for n in dom_names.iter().take(15) {
            eprintln!("      \"{n}\"");
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
    eprintln!(
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
    eprintln!("\n--deep: MonoImage.class_cache -> усі класи Assembly-CSharp (офсети підтверджено)");
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
            eprintln!("  прочитано таблицю з {n} бакетів");
            break;
        }
    }
    if buckets.is_empty() {
        eprintln!("не зміг прочитати жоден розмір таблиці з 0x{table_ptr:x}");
        return Vec::new();
    }

    // `class_cache` is a `MonoInternalHashTable`: it does not allocate its
    // own chain nodes, each bucket slot is only the *first* class hashed
    // there, and collisions chain through `MONO_CLASS_NEXT_CLASS_CACHE` on
    // the class itself (see that constant's doc comment for the evidence).
    // Reading only the bucket slot -- what an earlier version of this
    // function did -- silently drops every class chained behind the
    // first, `GameState` among them; that bug, not an absent class, is
    // why an earlier session concluded it didn't exist in this build.
    let mut classes: Vec<(String, u64)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &bucket in buckets.iter().filter(|&&b| plausible_ptr(b)) {
        let mut class_ptr = bucket;
        let mut hops = 0;
        loop {
            if !seen.insert(class_ptr) || hops > 40 {
                // Already visited (two buckets should never chain into
                // each other, but this is remote memory, not a trusted
                // in-process structure) or an implausibly long chain --
                // either way, stop rather than loop forever.
                break;
            }
            hops += 1;
            if let Some(np) = remote.read(class_ptr + mono_layout::MONO_CLASS_NAME, 8) {
                let name_ptr = u64::from_le_bytes(np.try_into().unwrap());
                if let Some(name) = read_cstring(remote, name_ptr, 96) {
                    if !name.is_empty() {
                        classes.push((name, class_ptr));
                    }
                }
            }
            let Some(nb) = remote.read(class_ptr + mono_layout::MONO_CLASS_NEXT_CLASS_CACHE, 8) else {
                break;
            };
            let next = u64::from_le_bytes(nb.try_into().unwrap());
            if !plausible_ptr(next) {
                break;
            }
            class_ptr = next;
        }
    }
    eprintln!(
        "  {} непорожніх бакетів, {} класів з них дали назву (з урахуванням ланцюжків колізій)",
        buckets.iter().filter(|&&b| plausible_ptr(b)).count(),
        classes.len()
    );
    let interesting: Vec<&str> = classes
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| INTERESTING_SUBSTRINGS.iter().any(|kw| n.contains(kw)))
        .collect();
    if !interesting.is_empty() {
        eprintln!("\n  !!! класи, що збігаються з ключовими словами:");
        for n in &interesting {
            eprintln!("    {n}");
        }
    }
    // `INTERESTING_SUBSTRINGS` is a guess at what a relevant class is
    // named, and guesses miss -- `GameState`/`GameMgr`/`GameLogic`/
    // `ZoneMgr` all turned out not to exist under those names at all
    // (2026-09-02). Opt-in, not printed by default: 16k names is not
    // something to scroll past on an ordinary run, but a real file to
    // grep against beats guessing another substring blind.
    if let Ok(path) = std::env::var("MEMREADER_DUMP_CLASSES") {
        let mut sorted: Vec<&str> = classes.iter().map(|(n, _)| n.as_str()).collect();
        sorted.sort_unstable();
        let _ = std::fs::write(&path, sorted.join("\n"));
        eprintln!("  (діагностика: усі {} назви класів записано в {path})", sorted.len());
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
///
/// `candidate == class_ptr` is excluded outright: a real object instance
/// can never legitimately share an address with its own `MonoClass*` --
/// seen live against `SceneMgr`, where dozens of candidates in its
/// static blob all "matched" this way (some reflection/type-metadata
/// array aliasing the class descriptor itself, not a real singleton
/// field), and no other class scanned this session produced it.
///
/// `verbose` prints the same per-candidate detail `--deep` always has --
/// off for `scan_all_for_self_typed_statics`, which calls this hundreds
/// of times and would otherwise bury a real hit under noise from classes
/// that were never going to be it.
fn find_self_typed_static(remote: &Remote, class_ptr: u64, vtable: u64, verbose: bool) -> Vec<u64> {
    let vtable_size = remote
        .read(class_ptr + mono_layout::MONO_CLASS_VTABLE_SIZE, 4)
        .map(|vs| i32::from_le_bytes(vs.try_into().unwrap()).max(0) as u64)
        .unwrap_or(0);
    if verbose {
        eprintln!(
            "    vtable_size (MonoClass+0x{:x}) = {vtable_size}",
            mono_layout::MONO_CLASS_VTABLE_SIZE
        );
    }

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
    if verbose {
        eprintln!(
            "    обчислений старт статичних даних: 0x{computed:x}; ширший діапазон: \
             0x{wide_start:x}..+0x{WIDE_LEN:x}"
        );
    }

    let Some(blob) = remote.read(wide_start, WIDE_LEN) else {
        if verbose {
            eprintln!("    не зміг прочитати навіть ширший діапазон з 0x{wide_start:x}");
        }
        return Vec::new();
    };
    let mut hits = Vec::new();
    for (i, chunk) in blob.chunks_exact(8).enumerate() {
        let candidate = u64::from_le_bytes(chunk.try_into().unwrap());
        if !plausible_ptr(candidate) || candidate == class_ptr {
            continue;
        }
        let Some(vt_bytes) = remote.read(candidate, 8) else { continue };
        let obj_vtable = u64::from_le_bytes(vt_bytes.try_into().unwrap());
        if !plausible_ptr(obj_vtable) {
            continue;
        }
        let Some(kb) = remote.read(obj_vtable + mono_layout::MONO_VTABLE_KLASS, 8) else { continue };
        if u64::from_le_bytes(kb.try_into().unwrap()) == class_ptr {
            if verbose {
                let off = i as u64 * 8;
                eprintln!(
                    "    vtable+0x48+0x{off:x} (абс. 0x{:x}): 0x{candidate:x} -> \
                     vtable 0x{obj_vtable:x} -> klass збігається!",
                    wide_start + off
                );
            }
            hits.push(candidate);
        }
    }
    hits
}

/// Every class with a resolvable vtable, scanned for a static field
/// holding one of `targets` verbatim -- not "an instance of the same
/// class" (`find_self_typed_static`'s question) but "who points at this
/// *specific*, already-known-live object". Strictly stronger evidence
/// when `targets` are addresses a heap scan already trusts (the current
/// match's own `GameEntity`/`Entity` id 2/3): the class doing the
/// pointing doesn't have to be guessed at all, unlike every
/// `find_singletons`/`scan_all_for_self_typed_statics` candidate name
/// tried so far.
fn scan_all_statics_for_addresses(
    remote: &Remote,
    classes: &[(String, u64)],
    targets: &[u64],
) -> Vec<(String, u64, u64)> {
    let mut found = Vec::new();
    let mut with_vtable = 0usize;
    let mut blobs_read = 0usize;
    for (name, class_ptr) in classes {
        let Some(vtable) = find_vtable(remote, *class_ptr) else {
            continue;
        };
        with_vtable += 1;
        // The same wide window `find_self_typed_static` scans (hypothesis
        // (b) there) rather than the narrower `vtable_size`-computed one --
        // an exact-value match is strong enough evidence on its own that
        // there's no need to also gamble on `vtable_size` being right.
        let wide_start = vtable + mono_layout::MONO_VTABLE_VTABLE_ARRAY;
        const WIDE_LEN: usize = 0x3000;
        let Some(blob) = remote.read(wide_start, WIDE_LEN) else {
            continue;
        };
        blobs_read += 1;
        for chunk in blob.chunks_exact(8) {
            let val = u64::from_le_bytes(chunk.try_into().unwrap());
            if targets.contains(&val) {
                found.push((name.clone(), *class_ptr, val));
            }
        }
    }
    eprintln!(
        "\n--deep: {with_vtable}/{} класів мали vtable, {blobs_read} статичних блобів прочитано. \
         Посилання на відомі живі адреси {targets:x?}:",
        classes.len()
    );
    if found.is_empty() {
        eprintln!("  жодне статичне поле жодного класу з vtable не тримає жодну з них.");
    }
    for (name, class_ptr, val) in &found {
        eprintln!("  {name} (MonoClass* = 0x{class_ptr:x}) тримає 0x{val:x}");
    }
    found
}

/// Every class with a resolvable vtable, scanned for a self-typed static
/// -- not just the fixed `TARGETS` list `find_singletons` tries. A guess
/// at what the right class is *named* has already missed twice
/// (`GameState`/`GameMgr`/`GameLogic`/`ZoneMgr` don't exist under those
/// names in this build at all); this doesn't guess a name, it looks at
/// every candidate the class cache actually has.
///
/// Only classes with between 1 and `MAX_CLEAN_HITS` hits are kept: zero
/// means nothing to report, and a large count (`SceneMgr` had several
/// dozen) is the same static-array/reflection noise pattern seen there,
/// not a plausible "the one live instance" singleton field.
fn scan_all_for_self_typed_statics(
    remote: &Remote,
    classes: &[(String, u64)],
) -> Vec<(String, u64, Vec<u64>)> {
    const MAX_CLEAN_HITS: usize = 3;
    let mut found = Vec::new();
    let mut with_vtable = 0usize;
    for (name, class_ptr) in classes {
        let Some(vtable) = find_vtable(remote, *class_ptr) else {
            continue;
        };
        with_vtable += 1;
        let hits = find_self_typed_static(remote, *class_ptr, vtable, false);
        if !hits.is_empty() && hits.len() <= MAX_CLEAN_HITS {
            found.push((name.clone(), *class_ptr, hits));
        }
    }
    eprintln!(
        "\n--deep: повне сканування self-typed static -- {with_vtable}/{} класів мали vtable, \
         {} дали 1..={MAX_CLEAN_HITS} влучань (не шум):",
        classes.len(),
        found.len()
    );
    for (name, class_ptr, hits) in &found {
        eprintln!("  {name} (MonoClass* = 0x{class_ptr:x}): {hits:x?}");
    }
    found
}

/// For every class this session's chat named as a live-tracking target,
/// try to find a static field holding an instance of itself -- the
/// pattern every C# singleton (`Foo.Get()`, `Foo.Instance`, ...) compiles
/// down to.
///
/// `GameState`/`GameMgr`/`GameLogic`/`ZoneMgr` -- report A.4's public
/// HearthMirror-derived names -- do not exist under those exact names in
/// this build's `class_cache` (confirmed 2026-09-02 against a live match
/// via `MEMREADER_DUMP_CLASSES`, not merely absent from this file's
/// keyword filter): the class names those names' era of reverse
/// engineering assumed have evidently changed since. `Player` DID
/// resolve one stable, single (not a repeated-noise pattern the way
/// `SceneMgr`'s hits are -- see below) static field across four
/// consecutive runs, at a fixed address, with `playerId == 1` -- but its
/// address does not match the current match's own live `Player` object
/// (cross-checked against a same-session `--snapshot`'s `players[]`),
/// and its `name` does not decode. Ruled out as this match's live
/// Player, not chased further: most likely some other Player-typed
/// reference (account/profile-level, not per-match) that this file has
/// no independent way to identify. `SceneMgr` resolved dozens of "hits"
/// that all point back at its own `MonoClass*` rather than a normal
/// object instance -- almost certainly reflection/type-metadata noise,
/// not a real singleton field; not investigated further.
///
/// Returns `(class_name, class_ptr, vtable_ptr)` for every class
/// a `MonoVTable*` was found for, whether or not a singleton was, since
/// the vtable pointer itself is exactly what a heap scan for live
/// instances needs next (see `scan_heap_for_class`).
fn find_singletons(remote: &Remote, classes: &[(String, u64)]) -> Vec<(String, u64, u64)> {
    const TARGETS: &[&str] = &[
        "DraftManager",
        "PowerProcessor",
        "GameState",
        "Entity",
        "Player",
        "GameEntity",
        "GameMgr",
        "GameLogic",
        "ZoneMgr",
        "InputManager",
        "SceneMgr",
        "TurnStartManager",
    ];
    eprintln!("\n--deep: пошук статичних синглтонів для {TARGETS:?}");
    let mut resolved = Vec::new();
    for &target in TARGETS {
        let Some(&(_, class_ptr)) = classes.iter().find(|(n, _)| n == target) else {
            eprintln!("  {target}: класу з такою точною назвою немає в дампі");
            continue;
        };
        eprintln!("  {target}: MonoClass* = 0x{class_ptr:x}");
        let Some(vtable) = find_vtable(remote, class_ptr) else {
            eprintln!(
                "    немає vtable (runtime_info порожній або жоден із перших \
                 4 слотів домену не пройшов перевірку klass) -- або клас ще \
                 не використовувався рушієм, або MONO_CLASS_RUNTIME_INFO у \
                 mono_layout.rs (обчислений, не підтверджений) невірний"
            );
            continue;
        };
        eprintln!("    MonoVTable* = 0x{vtable:x}");
        resolved.push((target.to_string(), class_ptr, vtable));
        let hits = find_self_typed_static(remote, class_ptr, vtable, true);
        if hits.is_empty() {
            eprintln!(
                "    жодного статичного поля з екземпляром цього ж типу в \
                 ширшому діапазоні -- ймовірно, синглтон зберігається інакше \
                 (ServiceLocator, а не static-поле)."
            );
        }
        // A structural match (vtable->klass == class_ptr) only proves the
        // hit is *an* instance of the right class, not that it is the
        // live match's own -- reading the same curated fields the normal
        // heap scan already trusts turns "found a hit" into something a
        // human (or a `--snapshot` cross-check) can actually judge.
        if target == "Player" {
            for &addr in &hits {
                let name = remote
                    .read(addr + mono_layout::PLAYER_NAME, 8)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .and_then(|p| try_mono_string(remote, p));
                let pid = remote
                    .read(addr + mono_layout::PLAYER_PLAYER_ID, 4)
                    .map(|b| i32::from_le_bytes(b.try_into().unwrap()));
                eprintln!("    Player-екземпляр 0x{addr:x}: name={name:?} playerId={pid:?}");
            }
        }
        if target == "Entity" {
            for &addr in &hits {
                let card_id = remote
                    .read(addr + mono_layout::ENTITY_CARD_ID, 8)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .and_then(|p| try_mono_string(remote, p));
                let eid = remote
                    .read(addr + mono_layout::ENTITY_ID, 4)
                    .map(|b| i32::from_le_bytes(b.try_into().unwrap()));
                eprintln!("    Entity-екземпляр 0x{addr:x}: cardId={card_id:?} id={eid:?}");
            }
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
fn scan_heap_for_class(
    remote: &Remote,
    pid: u32,
    class_name: &str,
    vtable: u64,
    max_hits: usize,
) -> Vec<u64> {
    eprintln!("\n--deep: сканую купчу пам'яті на живі об'єкти {class_name} (vtable=0x{vtable:x})");
    let Ok(maps) = procfs::read_maps(pid) else {
        eprintln!("не зміг прочитати /proc/{pid}/maps");
        return Vec::new();
    };
    let pattern = vtable.to_le_bytes();
    const CHUNK: u64 = 4 * 1024 * 1024;
    const MAX_SCAN_BYTES: u64 = 3_000_000_000;
    let max_hits = max_hits.max(1);

    let mut scanned: u64 = 0;
    let mut hits: Vec<u64> = Vec::new();
    let regions: Vec<&procfs::MapEntry> = maps
        .iter()
        .filter(|m| m.pathname.is_empty() && m.perms.starts_with("rw"))
        .collect();
    eprintln!("  {} анонімних rw-регіонів у мапі процесу", regions.len());

    'outer: for region in &regions {
        let mut addr = region.start;
        while addr < region.end {
            if scanned >= MAX_SCAN_BYTES || hits.len() >= max_hits {
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
                        if hits.len() >= max_hits {
                            break;
                        }
                    }
                    i += 8;
                }
            }
            addr += len as u64;
        }
    }

    eprintln!(
        "  проскановано {:.0} МБ, {} збігів{}",
        scanned as f64 / 1_048_576.0,
        hits.len(),
        if hits.len() >= max_hits { " (досягнуто ліміту)" } else { "" }
    );
    for &h in hits.iter().take(20) {
        eprintln!("    {class_name}* кандидат: 0x{h:x}");
    }
    if hits.is_empty() {
        eprintln!(
            "жодного об'єкта не знайдено. Можливо, зараз немає активної гри/\
             екрана з такими об'єктами (спробуйте під час бою чи драфту), або \
             ці анонімні rw-регіони — не той пул пам'яті, де Mono тримає купу \
             (Boehm GC іноді резервує пам'ять нетиповим способом під Wine)."
        );
    } else {
        eprintln!(
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
    eprintln!("\n--deep: сирий дамп полів {class_name}* = 0x{addr:x}");
    const DUMP_LEN: usize = 0x180;
    let Some(bytes) = remote.read(addr, DUMP_LEN) else {
        eprintln!("не зміг прочитати 0x{DUMP_LEN:x} байтів з 0x{addr:x}");
        return;
    };

    eprintln!("  сирі байти:");
    for (row, chunk) in bytes.chunks(16).enumerate() {
        eprintln!("    +0x{:03x}: {}", row * 16, hex(chunk));
    }

    eprintln!("  рядки, знайдені через вказівники в полях (char* ascii):");
    let mut any_string = false;
    for off in (8..DUMP_LEN - 8).step_by(8) {
        let ptr = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        if !plausible_ptr(ptr) {
            continue;
        }
        let Some(str_bytes) = remote.read(ptr, 64) else { continue };
        if let Some(s) = looks_like_name(&str_bytes) {
            eprintln!("    +0x{off:x}: \"{s}\"");
            any_string = true;
        }
    }
    if !any_string {
        eprintln!("    (жодної)");
    }

    eprintln!("  рядки, знайдені через вказівники в полях (System.String / MonoString, UTF-16):");
    let mut any_mono_string = false;
    for off in (8..DUMP_LEN - 8).step_by(8) {
        let ptr = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        if let Some(s) = try_mono_string(remote, ptr) {
            eprintln!("    +0x{off:x}: \"{s}\"");
            any_mono_string = true;
        }
    }
    if !any_mono_string {
        eprintln!("    (жодної)");
    }

    eprintln!("  малі 32-бітні числа (кандидати на id/тег, діапазон 1..=2000):");
    let mut any_int = false;
    for off in (8..DUMP_LEN - 4).step_by(4) {
        let v = i32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        if (1..=2000).contains(&v) {
            eprintln!("    +0x{off:x}: {v}");
            any_int = true;
        }
    }
    if !any_int {
        eprintln!("    (жодного)");
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
fn collect_entity_hits(remote: &Remote, hits: &[u64]) -> Vec<EntityHit> {
    let mut rows = Vec::new();
    for &addr in hits {
        let Some(bytes) = remote.read(addr, mono_layout::ENTITY_INSTANCE_SIZE as usize) else { continue };
        let card_off = mono_layout::ENTITY_CARD_ID as usize;
        let id_off = mono_layout::ENTITY_ID as usize;
        let card_ptr = u64::from_le_bytes(bytes[card_off..card_off + 8].try_into().unwrap());
        let card_id = try_mono_string(remote, card_ptr).filter(|s| looks_like_card_id(s));
        let id_field = i32::from_le_bytes(bytes[id_off..id_off + 4].try_into().unwrap());
        let tags = read_entity_tags(remote, addr);
        rows.push(EntityHit {
            addr,
            card_id,
            id: id_field,
            tags,
        });
    }
    rows
}

fn dump_entity_table(remote: &Remote, hits: &[u64]) -> Vec<(u64, Option<String>, i32)> {
    eprintln!(
        "\n--deep: таблиця Entity (cardId @ +0x{:x}, id @ +0x{:x}, підтверджено live) для всіх {} знайдених об'єктів",
        mono_layout::ENTITY_CARD_ID, mono_layout::ENTITY_ID, hits.len()
    );
    let collected = collect_entity_hits(remote, hits);
    let rows: Vec<(u64, Option<String>, i32)> = collected
        .iter()
        .map(|e| (e.addr, e.card_id.clone(), e.id))
        .collect();
    for e in &collected {
        eprintln!(
            "  0x{:x}: cardId={:<20} id={} zone={:?} controller={:?}",
            e.addr,
            e.card_id.as_deref().unwrap_or("?"),
            e.id,
            e.tag(TAG_ZONE),
            e.tag(TAG_CONTROLLER)
        );
    }
    const INTERESTING: &[&str] = &[
        "EDR_654",
        "CORE_ULD_723",
        "JAIL_912",
        "HERO_09dbp",
        "HERO_11bpt",
    ];
    let mut inspected = 0usize;
    for prefix in INTERESTING {
        if let Some((addr, card_id, id_field)) = rows
            .iter()
            .find(|(_, c, _)| c.as_deref().map(|s| s.starts_with(prefix)).unwrap_or(false))
        {
            let cid = card_id.as_deref().unwrap_or(prefix);
            eprintln!(
                "\n  !!! {cid} @ 0x{addr:x}  EntityID(+0x{:x})={id_field}",
                mono_layout::ENTITY_ID
            );
            inspect_entity_tag_pointers(remote, *addr, cid);
            inspected += 1;
            if inspected >= 3 {
                break;
            }
        }
    }
    if inspected == 0 {
        if let Some((addr, card_id, id_field)) = rows
            .iter()
            .find(|(_, c, id)| c.as_ref().is_some() && (20..400).contains(id))
        {
            let cid = card_id.as_deref().unwrap_or("?");
            eprintln!(
                "\n  (жодної з відомих карток — беру першу з правдоподібним id: {cid} id={id_field} @ 0x{addr:x})"
            );
            inspect_entity_tag_pointers(remote, *addr, cid);
        }
    }
    rows
}

/// Same idea as `dump_entity_table`, for `Player` -- `PLAYER_NAME` and
/// `PLAYER_PLAYER_ID` are both confirmed live now on 20+ objects (see
/// `mono_layout.rs`).
fn collect_player_hits(remote: &Remote, hits: &[u64]) -> Vec<PlayerHit> {
    const WINDOW: usize = 0x200;
    let mut rows = Vec::new();
    for &addr in hits {
        let Some(bytes) = remote.read(addr, WINDOW) else { continue };
        let name_off = mono_layout::PLAYER_NAME as usize;
        let id_off = mono_layout::PLAYER_PLAYER_ID as usize;
        let name_ptr = u64::from_le_bytes(bytes[name_off..name_off + 8].try_into().unwrap());
        let name = try_mono_string(remote, name_ptr);
        let player_id = i32::from_le_bytes(bytes[id_off..id_off + 4].try_into().unwrap());
        // No confirmed offset for a Player's own EntityID -- see the
        // field's doc comment on `PlayerHit`. `mono_layout::ENTITY_ID`
        // (+0x38) is an `Entity` offset, not `Player`'s, and reading it
        // here was reading nonsense (~1e9 on a real pair, negative garbage
        // on a stale one). Left `None` rather than emitting a number that
        // looks like data but isn't.
        let entity_id: Option<i32> = None;
        let tags = read_entity_tags(remote, addr);
        rows.push(PlayerHit {
            addr,
            name,
            player_id,
            entity_id,
            tags,
        });
    }
    rows
}

fn dump_player_table(remote: &Remote, hits: &[u64]) -> Vec<PlayerHit> {
    eprintln!(
        "\n--deep: таблиця Player (ім'я @ +0x{:x}, playerId @ +0x{:x}, підтверджено live) для всіх {} знайдених об'єктів",
        mono_layout::PLAYER_NAME, mono_layout::PLAYER_PLAYER_ID, hits.len()
    );
    let rows = collect_player_hits(remote, hits);
    for p in &rows {
        eprintln!(
            "  0x{:x}: ім'я={:<20} playerId={} entityId={}",
            p.addr,
            p.name.as_deref().unwrap_or("?"),
            p.player_id,
            p.entity_id.map(|n| n.to_string()).unwrap_or("?".into())
        );
    }
    rows
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
    eprintln!("\n--deep: 4 невідомі вказівники {label} (0x{addr:x})");
    let Some(obj) = remote.read(addr, mono_layout::ENTITY_INSTANCE_SIZE as usize) else {
        eprintln!("  не зміг прочитати сам об'єкт");
        return;
    };
    for &off in &mono_layout::ENTITY_UNKNOWN_PTRS {
        let off = off as usize;
        let ptr = u64::from_le_bytes(obj[off..off + 8].try_into().unwrap());
        let cname = class_name_of(remote, ptr).unwrap_or_else(|| "?".into());
        let mark = if off as u64 == mono_layout::ENTITY_TAG_LIST {
            "  (live tags)"
        } else {
            ""
        };
        eprintln!("  +0x{off:x}: 0x{ptr:x}  class={cname}{mark}");
        if !plausible_ptr(ptr) {
            continue;
        }
        dump_dotnet_list(remote, ptr);
    }
}

/// `List<T>`: `T[] _items` at +0x10, `int _size` at +0x18. `T[]` is a
/// `MonoArray` (`max_length` at +0x18, vector at +0x20). For class `T`
/// the vector is pointers; `Tag` is a class with two ints after the
/// 16-byte object header (`Name` at +0x10, `Value` at +0x14).
fn dump_dotnet_list(remote: &Remote, list: u64) {
    let Some(hdr) = remote.read(list, 0x20) else { return };
    let items = u64::from_le_bytes(hdr[0x10..0x18].try_into().unwrap());
    let size = i32::from_le_bytes(hdr[0x18..0x1c].try_into().unwrap());
    let items_name = class_name_of(remote, items).unwrap_or_else(|| "?".into());
    eprintln!("    List._size={size}  _items=0x{items:x} class={items_name}");
    if !(1..=256).contains(&size) || !plausible_ptr(items) {
        return;
    }
    let Some(arr) = remote.read(items, 0x20) else { return };
    let max_len = i32::from_le_bytes(arr[0x18..0x1c].try_into().unwrap());
    eprintln!("    array.max_length={max_len}");
    let n = size as usize;
    let Some(vecb) = remote.read(items + 0x20, n * 8) else { return };
    let mut printed = 0usize;
    for i in 0..n {
        let elem = u64::from_le_bytes(vecb[i * 8..i * 8 + 8].try_into().unwrap());
        if !plausible_ptr(elem) {
            continue;
        }
        let Some(tb) = remote.read(elem, 0x18) else { continue };
        let name = i32::from_le_bytes(
            tb[mono_layout::TAG_NAME as usize..mono_layout::TAG_NAME as usize + 4]
                .try_into()
                .unwrap(),
        );
        let value = i32::from_le_bytes(
            tb[mono_layout::TAG_VALUE as usize..mono_layout::TAG_VALUE as usize + 4]
                .try_into()
                .unwrap(),
        );
        let known = game_tag_name(name);
        if known.is_some() || printed < 8 {
            eprintln!(
                "      [{}] Tag name={name}{} value={value}",
                i,
                known.map(|s| format!(" ({s})")).unwrap_or_default()
            );
            printed += 1;
        }
    }
}

fn game_tag_name(k: i32) -> Option<&'static str> {
    Some(match k {
        19 => "STEP",
        20 => "TURN",
        23 => "CURRENT_PLAYER",
        25 => "RESOURCES_USED",
        26 => "RESOURCES",
        27 => "HERO_ENTITY",
        28 => "MAXHANDSIZE",
        29 => "STARTHANDSIZE",
        30 => "PLAYER_ID",
        31 => "TEAM_ID",
        45 => "HEALTH",
        47 => "ATK",
        48 => "COST",
        49 => "ZONE",
        50 => "CONTROLLER",
        53 => "ENTITY_ID",
        176 => "MAXRESOURCES",
        199 => "CLASS",
        202 => "CARDTYPE",
        263 => "ZONE_POSITION",
        292 => "ARMOR",
        313 => "PREMIUM",
        _ => return None,
    })
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

/// Unit tests for the pure reconstruction logic (`game_entity`,
/// `dedup_by_id`, `current_game_entities`, `build_sides`, ...) -- built by
/// hand from `EntityHit`/`PlayerHit`, no running process needed. This is
/// deliberately the one part of `memreader` that *can* be tested this way:
/// everything upstream of these functions (PID discovery, PE parsing, the
/// heap scan itself) genuinely needs a real Hearthstone process and can't
/// be exercised offline, which is why this file had zero tests until now.
///
/// Several of these encode addresses and tag shapes taken straight from
/// real live dumps this session (see the commit history and
/// `memreader/README.md`'s "known, not yet resolved bug" note) -- close
/// enough to reality to be worth keeping as regressions, not just
/// hypothetical shapes.
#[cfg(test)]
mod tests {
    use super::*;

    fn entity(id: i32, addr: u64, tags: &[(i32, i32)]) -> EntityHit {
        EntityHit {
            addr,
            card_id: None,
            id,
            tags: tags.to_vec(),
        }
    }

    fn player(name: &str, player_id: i32, addr: u64) -> PlayerHit {
        // No test constructs a Player with a real entity_id any more --
        // nothing reads it (see `PlayerHit::entity_id`'s doc comment), so
        // the fixture doesn't take one either.
        PlayerHit {
            addr,
            name: Some(name.to_string()),
            player_id,
            entity_id: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn game_entity_prefers_a_candidate_that_actually_carries_turn_or_step() {
        // The stale one has more nearby "players" (a fuller, finished
        // board leaves more resident garbage) -- exactly the shape that
        // fooled the old nearby-count-only heuristic, per the live report
        // this fix was written for.
        let stale_game = entity(1, 0x1000_0000, &[(TAG_CARDTYPE, 1)]);
        let stale_p2 = entity(2, 0x1000_1000, &[(TAG_CONTROLLER, 1)]);
        let stale_p3 = entity(3, 0x1000_2000, &[(TAG_CONTROLLER, 2)]);
        let stale_extra = entity(4, 0x1000_3000, &[(TAG_CONTROLLER, 1)]);

        let live_game = entity(1, 0x9000_0000, &[(TAG_TURN, 9), (TAG_STEP, 10)]);
        let live_p2 = entity(2, 0x9000_1000, &[(TAG_CONTROLLER, 1)]);
        let live_p3 = entity(3, 0x9000_2000, &[(TAG_CONTROLLER, 2)]);

        let hits = vec![
            stale_game, stale_p2, stale_p3, stale_extra, live_game, live_p2, live_p3,
        ];
        let picked = game_entity(&hits).expect("a candidate");
        assert_eq!(picked.addr, 0x9000_0000, "the live-tagged one, not the more crowded stale one");
    }

    #[test]
    fn game_entity_falls_back_to_nearby_player_count_when_nothing_is_tagged() {
        // Neither candidate carries TURN/STEP (the common case -- see
        // README's note that this doesn't always help) and neither shows
        // any hero topology either (so `game_entity_score`'s first two
        // tuple fields tie at (-8, 0) for both), so the third field --
        // nearby id==2/3 count, the original pre-PR2 heuristic, now
        // demoted to a documented last-resort tie-break -- has to do the
        // deciding. This is a deliberate third priority now (see
        // `game_entity_score`'s doc comment), not the accidental
        // "whichever the scan happened to list last" that `max_by_key`
        // would otherwise silently fall back to with no third field at
        // all.
        let sparse = entity(1, 0x1000_0000, &[]);
        let sparse_p2 = entity(2, 0x1000_1000, &[]);

        let crowded = entity(1, 0x2000_0000, &[]);
        let crowded_p2 = entity(2, 0x2000_1000, &[]);
        let crowded_p3 = entity(3, 0x2000_2000, &[]);

        let hits = vec![sparse, sparse_p2, crowded, crowded_p2, crowded_p3];
        let picked = game_entity(&hits).expect("a candidate");
        assert_eq!(picked.addr, 0x2000_0000, "two nearby players beats one");
    }

    #[test]
    fn game_entity_prefers_legal_topology_over_a_fuller_tagged_stale_board() {
        // A2/A3 of the report: raw neighbour count (and even a stale
        // TURN/STEP tag pair left over in memory) is not proof of "this
        // is the live game". A fuller island -- more entities nearby, and
        // still tagged TURN/STEP from before its match ended -- but with
        // an illegal board (no hero for either controller, and one
        // controller over the 7-minion legal maximum) must lose to a
        // smaller, thinner island that is topologically a real legal
        // board (a hero in PLAY for both controllers), even though that
        // one carries no TURN/STEP at all.
        let stale_game = entity(1, 0x1000_0000, &[(TAG_TURN, 9), (TAG_STEP, 10)]);
        let mut stale_hits = vec![stale_game];
        // No hero for either controller anywhere nearby -- and controller
        // 1 has 8 PLAY-zone minions, one past the legal maximum.
        for i in 0..8 {
            stale_hits.push(entity(
                10 + i,
                0x1000_0000 + 0x1000 * (i as u64 + 1),
                &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_MINION)],
            ));
        }

        let live_game = entity(1, 0x9000_0000, &[]);
        let live_hero_p1 = entity(
            2,
            0x9000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let live_hero_p2 = entity(
            3,
            0x9000_2000,
            &[(TAG_CONTROLLER, 2), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );

        let mut hits = stale_hits;
        hits.push(live_game);
        hits.push(live_hero_p1);
        hits.push(live_hero_p2);

        let picked = game_entity(&hits).expect("a candidate");
        assert_eq!(
            picked.addr, 0x9000_0000,
            "smaller, untagged but topologically legal board beats a fuller, tagged, illegal one"
        );
    }

    #[test]
    fn game_entity_prefers_legal_topology_when_neither_candidate_is_tagged() {
        // The exact live shape from the report: neither `id==1` candidate
        // carries TURN/STEP at all, including the one that actually held
        // the correct live Players -- so topology has to be the whole
        // signal, not just a tiebreaker among tagged candidates.
        //
        // This also has to be a *real* regression check against the old,
        // pre-topology-scoring algorithm (raw count of nearby id==2/3
        // within 0x10_0000, gated only by TURN/STEP presence), not an
        // accidental pass: with only `bad_hero_p1`/`bad_p2` and
        // `good_hero_p1`/`good_hero_p2` as the sole id==2/3 entities, both
        // islands would tie at a nearby-count of 2 under the old
        // algorithm, and `Iterator::max_by_key` breaks ties by keeping the
        // *last* equally-maximal element -- which happens to be `good`
        // simply because it is pushed later into `hits`, so the old code
        // would "pass" this fixture too, for a reason that has nothing to
        // do with topology. The extra untagged `bad_noise_*` entities
        // below give the *old* algorithm's nearby-id-2/3 count a clear,
        // non-tied edge for `bad` (5 vs 2) -- reproducing the report's
        // literal "ranking by nearby-player-count alone picked the stale
        // island" failure -- while contributing nothing to the *new*
        // algorithm's topology score (no CONTROLLER/CARDTYPE/ZONE tags),
        // so `good` still has to win on topology alone, not by a leftover
        // tie-break coincidence.
        let bad_game = entity(1, 0x1000_0000, &[]);
        // Controller 1 has a hero; controller 2 has none anywhere nearby --
        // not a legal Hearthstone board past mulligan.
        let bad_hero_p1 = entity(
            2,
            0x1000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let bad_p2 = entity(3, 0x1000_2000, &[(TAG_CONTROLLER, 2)]);
        let bad_noise_1 = entity(2, 0x1000_3000, &[]);
        let bad_noise_2 = entity(3, 0x1000_4000, &[]);
        let bad_noise_3 = entity(2, 0x1000_5000, &[]);

        let good_game = entity(1, 0x9000_0000, &[]);
        let good_hero_p1 = entity(
            2,
            0x9000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let good_hero_p2 = entity(
            3,
            0x9000_2000,
            &[(TAG_CONTROLLER, 2), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let good_minion = entity(
            4,
            0x9000_3000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_MINION)],
        );

        let hits = vec![
            bad_game, bad_hero_p1, bad_p2, bad_noise_1, bad_noise_2, bad_noise_3, good_game,
            good_hero_p1, good_hero_p2, good_minion,
        ];
        let picked = game_entity(&hits).expect("a candidate");
        assert_eq!(
            picked.addr, 0x9000_0000,
            "the legal two-hero board wins even with no TURN/STEP anywhere, and even though \
             the old raw-neighbour-count heuristic would have picked the other one outright"
        );
    }

    #[test]
    fn game_entity_never_lets_tag_presence_override_a_real_topology_difference() {
        // The core of the report's finding on this function: an earlier
        // cut scored TURN/STEP as a flat +5/+5 added directly into the
        // same total as the ±4-per-controller hero signal, so 10 points of
        // (unreliable -- Boehm/bdwgc leaves a finished match's tags
        // exactly as they were) tag presence could outvote an 8-point
        // topology difference -- here, a stale island missing a hero for
        // one controller (score 0) beats a live island with a confirmed
        // hero for *both* controllers (score 8) purely because the stale
        // one happens to still carry TURN/STEP. `game_entity_score`'s
        // tuple ordering makes that structurally impossible: topology is
        // compared first, and 0 < 8 regardless of what either candidate's
        // tag_bonus is.
        let stale_game = entity(1, 0x1000_0000, &[(TAG_TURN, 9), (TAG_STEP, 10)]);
        let stale_hero_p1 = entity(
            2,
            0x1000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        // Controller 2's hero is missing from this island entirely.
        let stale_p2_no_hero = entity(3, 0x1000_2000, &[(TAG_CONTROLLER, 2)]);

        let live_game = entity(1, 0x9000_0000, &[]);
        let live_hero_p1 = entity(
            2,
            0x9000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let live_hero_p2 = entity(
            3,
            0x9000_2000,
            &[(TAG_CONTROLLER, 2), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );

        let hits = vec![
            stale_game, stale_hero_p1, stale_p2_no_hero, live_game, live_hero_p1, live_hero_p2,
        ];
        let picked = game_entity(&hits).expect("a candidate");
        assert_eq!(
            picked.addr, 0x9000_0000,
            "a real topology gap (one controller's hero missing) must win over TURN/STEP tags, \
             not lose to them"
        );
    }

    #[test]
    fn topology_confidence_is_high_when_both_heroes_are_confirmed() {
        let game = entity(1, 0x1000_0000, &[]);
        let hero_p1 = entity(
            2,
            0x1000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let hero_p2 = entity(
            3,
            0x1000_2000,
            &[(TAG_CONTROLLER, 2), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let live = vec![game, hero_p1, hero_p2];
        assert_eq!(topology_confidence(&live), "high");
    }

    #[test]
    fn topology_confidence_is_low_when_a_hero_is_missing() {
        // Only controller 1's hero shows up nearby -- not a legal board,
        // but not disqualified outright (no >7-minion violation) either,
        // so `game_entity` still returns *a* candidate. The confidence
        // label is what tells a caller not to trust it fully, per report
        // A.7 PR1's "add confidence... when topology fails".
        let game = entity(1, 0x1000_0000, &[]);
        let hero_p1 = entity(
            2,
            0x1000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let live = vec![game, hero_p1];
        assert_eq!(topology_confidence(&live), "low");
    }

    #[test]
    fn topology_confidence_is_none_when_there_is_no_id_one_candidate_at_all() {
        let stray_minion = entity(
            10,
            0x1000_0000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_MINION)],
        );
        let live = vec![stray_minion];
        assert_eq!(topology_confidence(&live), "none");
    }

    #[test]
    fn game_entity_beats_the_old_raw_neighbour_count_heuristic_when_untagged() {
        // Report A.2 point 2's literal failure mode, reproduced directly:
        // "take every id==1 and pick the one with the most nearby id 2/3" --
        // no tags anywhere, no board-legality signal checked at all, purely
        // a count of nearby Player-shaped entities. A stale, finished
        // match's heap island can carry more leftover Player-adjacent
        // garbage than a mulligan-fresh live game, so the raw-count
        // heuristic picks the *stale* island. Topology scoring must not:
        // the stale island here shows no hero for either controller (an
        // illegal/incomplete board), while the live one shows both.
        let stale_game = entity(1, 0x1000_0000, &[]);
        // Five untagged id==2/3 "leftover Player object" entities nearby --
        // plenty to win on raw count alone (old heuristic: whoever has the
        // most nearby id==2/3 within 0x10_0000), but none of them carry a
        // CONTROLLER/CARDTYPE/ZONE triple, so they contribute nothing to
        // the new topology score.
        let stale_noise: Vec<EntityHit> = (0..5)
            .map(|i| {
                entity(
                    if i % 2 == 0 { 2 } else { 3 },
                    0x1000_0000 + 0x1000 * (i as u64 + 1),
                    &[],
                )
            })
            .collect();

        let live_game = entity(1, 0x9000_0000, &[]);
        let live_hero_p1 = entity(
            2,
            0x9000_1000,
            &[(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );
        let live_hero_p2 = entity(
            3,
            0x9000_2000,
            &[(TAG_CONTROLLER, 2), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_HERO)],
        );

        let mut hits = vec![stale_game];
        hits.extend(stale_noise);
        hits.push(live_game);
        hits.push(live_hero_p1);
        hits.push(live_hero_p2);

        let picked = game_entity(&hits).expect("a candidate");
        assert_eq!(
            picked.addr, 0x9000_0000,
            "topologically legal live island beats a stale one that only wins on raw \
             neighbour count"
        );
    }

    #[test]
    fn dedup_by_id_keeps_the_more_filled_copy_over_a_closer_empty_one() {
        let anchor = 0x5000_0000u64;
        let empty_but_close = EntityHit {
            addr: anchor + 0x40,
            card_id: None,
            id: 7,
            tags: vec![],
        };
        let filled_but_far = EntityHit {
            addr: anchor + 0x1000_0000,
            card_id: Some("CS2_182".into()),
            id: 7,
            tags: vec![(TAG_ZONE, ZONE_PLAY)],
        };
        let deduped = dedup_by_id(&[empty_but_close, filled_but_far.clone()], anchor);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].addr, filled_but_far.addr, "card_id+zone beats mere proximity");
    }

    #[test]
    fn dedup_by_id_breaks_a_tie_in_fill_by_proximity_to_the_anchor() {
        let anchor = 0x5000_0000u64;
        let near = EntityHit {
            addr: anchor + 0x40,
            card_id: Some("CS2_182".into()),
            id: 7,
            tags: vec![(TAG_ZONE, ZONE_PLAY)],
        };
        let far = EntityHit {
            addr: anchor + 0x1000_0000,
            card_id: Some("CS2_182".into()),
            id: 7,
            tags: vec![(TAG_ZONE, ZONE_PLAY)],
        };
        let deduped = dedup_by_id(&[far, near.clone()], anchor);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].addr, near.addr, "equally filled -- the closer one wins");
    }

    #[test]
    fn current_game_entities_excludes_high_ids_far_outside_the_window() {
        let game = entity(1, 0x5000_0000, &[(TAG_TURN, 3)]);
        let near_minion = entity(10, 0x5000_1000, &[]);
        // Same id-space as a real leftover match's entities, but physically
        // in a different heap island entirely -- must not raise the cap.
        let far_leftover = entity(9000, 0x9000_0000_0000, &[]);
        let live = current_game_entities(&[game, near_minion, far_leftover]);
        assert!(live.iter().any(|e| e.id == 10), "the real neighbour is kept");
        assert!(!live.iter().any(|e| e.id == 9000), "the distant leftover is not");
    }

    #[test]
    fn build_sides_prefers_a_player_in_the_same_heap_island_over_a_closer_stale_one() {
        // The exact shape reported live: a same-`player_id`, correctly
        // named Player object sits in the *wrong*, distant heap island
        // (from an old, finished match) and is nonetheless the raw-nearest
        // one to the anchor; the real, live-game Player is farther away in
        // raw address terms but within `SAME_GAME_WINDOW` of the anchor
        // this game's entities actually live in.
        let anchor = 0x1ef7_0000u64;
        let game = entity(1, anchor, &[(TAG_TURN, 9), (TAG_STEP, 10)]);
        let live_p2 = entity(2, anchor + 0x1000, &[(TAG_CONTROLLER, 1)]);
        let live = vec![game, live_p2];

        let real = player("Seizan", 1, anchor + 0x3000); // within window
        // Outside the window, but still nearer in raw address terms than
        // `real` would be to a *different* anchor -- the fix has to reject
        // it for being out of window, not merely for being farther away
        // than some other candidate, or a case with only one in-window
        // candidate wouldn't be distinguishable from "no window match at
        // all". 64 MiB puts it solidly past `SAME_GAME_WINDOW` (32 MiB).
        let stale = player("Jafgaf", 1, anchor.wrapping_sub(0x0400_0000));

        let sides = build_sides(&live, &[stale, real], None);
        let side1 = sides.iter().find(|s| s.player_id == 1).expect("a side for pid 1");
        assert_eq!(side1.name.as_deref(), Some("Seizan"), "the live-island player, not the nearer stale one");
    }

    #[test]
    fn build_sides_refuses_to_name_a_side_from_outside_the_window() {
        // No live-window candidate exists at all for pid 1. The old
        // behaviour fell back to "nearest Player anywhere", which is
        // exactly the bug this report describes: a same-`player_id`,
        // correctly-named Player from a long-finished, unrelated match is
        // not "this match's player" just because nothing closer exists.
        // The fix: `name` stays `None` rather than borrowing a stranger's
        // battletag. The board itself (`play`/`hand`/... via CONTROLLER)
        // still must populate normally -- only the name is affected.
        let anchor = 0x1ef7_0000u64;
        let game = entity(1, anchor, &[(TAG_TURN, 9)]);
        let live_p2 = entity(2, anchor + 0x1000, &[(TAG_CONTROLLER, 1)]);
        let minion = EntityHit {
            addr: anchor + 0x2000,
            card_id: Some("CS2_182".into()), // `take()` requires a cardId for non-hero PLAY entities
            id: 10,
            tags: vec![(TAG_CONTROLLER, 1), (TAG_ZONE, ZONE_PLAY), (TAG_CARDTYPE, CARDTYPE_MINION)],
        };
        let live = vec![game, live_p2, minion];

        let far_a = player("Far1", 1, anchor + 0x1000_0000);
        let far_b = player("Far2", 1, anchor + 0x2000_0000);
        let sides = build_sides(&live, &[far_a, far_b], None);
        let side1 = sides.iter().find(|s| s.player_id == 1).expect("board entities keep the side alive");
        assert_eq!(side1.name, None, "no in-window Player -- must not borrow one from outside it");
        assert_eq!(side1.play.len(), 1, "board entities (via CONTROLLER/ZONE) are unaffected by the missing name");
    }

    #[test]
    fn find_tag_reads_the_first_matching_pair_and_none_when_absent() {
        let tags = [(TAG_ZONE, ZONE_HAND), (TAG_COST, 3)];
        assert_eq!(find_tag(&tags, TAG_ZONE), Some(ZONE_HAND));
        assert_eq!(find_tag(&tags, TAG_COST), Some(3));
        assert_eq!(find_tag(&tags, TAG_ATK), None);
    }

    #[test]
    fn looks_like_card_id_accepts_the_lowercase_suffixes_real_cards_have() {
        // The exact regression this fixed: HERO_09dbp is a real, live-
        // confirmed card id (see mono_layout.rs and this session's commit
        // history) that an uppercase-only filter silently dropped.
        for id in ["HERO_09dbp", "EDR_463b", "JAIL_430e1", "CATA_476t", "CS2_182"] {
            assert!(looks_like_card_id(id), "{id} should be accepted");
        }
        assert!(!looks_like_card_id("no_underscore".replace('_', "").as_str()));
        assert!(!looks_like_card_id("x"), "too short");
    }
}

