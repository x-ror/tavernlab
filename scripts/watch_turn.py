#!/usr/bin/env python3
"""Wait for the local player's next turn, print a board/hand snapshot, exit.

Coach checklist (must not regress):
  - attacks from board BEFORE spells/wipes; face if no taunt
  - leftover >=2 mana -> hero power UNLESS already at max HP and HP only heals
  - mulligan: dump 6+ on the play (Medivh/Karazhan/Azalina body)
  - COPY-PROC: 'while holding this' (Mind Sweeper AOE, Unshackle discount)
    is ON iff we played a card NOT in our original deck while that copy
    was already in hand. Azalina copies count. Do NOT guess from the board.
"""
import os, re, sys, time
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import console

def _default_logs():
    """Windows well-known path; POSIX has no standard place, so a
    Wine/Proton install has to say where it lives via HS_LOGS."""
    local = os.environ.get("LOCALAPPDATA")
    if local:
        return os.path.join(local, "Blizzard", "Hearthstone", "Logs")
    return ""


# All three are per-user and stay out of the repository: a battletag is
# in-game identity and a deckstring is a personal list.
#   HS_LOGS  — the Hearthstone Logs directory
#   HS_ME    — your battletag, e.g. Name#12345
#   HS_DECK  — your deckstring
LOGS = os.environ.get("HS_LOGS") or _default_logs()
ME = os.environ.get("HS_ME", "")
MY_DECK = os.environ.get("HS_DECK", "")
FACE_BURN = {
    "Moonwell": 4, "Lifedrinker": 3, "Fireball": 6, "Frostbolt": 3,
    "Mind Blast": 5, "Holy Smite": 3,
}
RE_HAND_LAYOUT = re.compile(r"END waiting for zone FRIENDLY HAND")
RE_PTAG = re.compile(
    r"TAG_CHANGE Entity=(\S+#\S+) tag=(RESOURCES|RESOURCES_USED|TURN|"
    r"CURRENT_PLAYER|PLAYSTATE) value=(\S+)")

RE_ZONE = re.compile(
    r"\[entityName=(.+?) id=(\d+) zone=\w+ zonePos=(\d+) cardId=(\S*) "
    r"player=(\d+)\] zone from .* -> (FRIENDLY|OPPOSING) "
    r"([A-Z]+)(?: \(([^)]+)\))?"
)
RE_FROM_HAND = re.compile(
    r"\[entityName=(.+?) id=(\d+) zone=\w+ zonePos=\d+ cardId=(\S*) "
    r"player=(\d+)\] zone from FRIENDLY HAND ->(?! FRIENDLY DECK)"
)
RE_TO_HAND = re.compile(
    r"\[entityName=(.+?) id=(\d+) zone=\w+ zonePos=\d+ cardId=(\S*) "
    r"player=(\d+)\] zone from .* -> FRIENDLY HAND"
)
if not ME:
    raise SystemExit(
        "Set HS_ME to your battletag (Name#12345); watch_turn.py keys "
        "every regex off it. HS_LOGS and HS_DECK are optional.")

RE_CUR = re.compile(rf"TAG_CHANGE Entity={re.escape(ME)} tag=CURRENT_PLAYER value=([01])")
RE_RES = re.compile(rf"TAG_CHANGE Entity={re.escape(ME)} tag=RESOURCES value=(\d+)")
RE_USED = re.compile(rf"TAG_CHANGE Entity={re.escape(ME)} tag=RESOURCES_USED value=(\d+)")
RE_TURN = re.compile(rf"TAG_CHANGE Entity={re.escape(ME)} tag=TURN value=(\d+)")
RE_STATE = re.compile(rf"TAG_CHANGE Entity={re.escape(ME)} tag=PLAYSTATE value=(\w+)")
RE_TAG_BR = re.compile(r"TAG_CHANGE Entity=\[.*?id=(\d+).*?\] tag=(\w+) value=(\S+)")
RE_TAG_ID = re.compile(r"TAG_CHANGE Entity=(\d+) tag=(\w+) value=(\S+)")


def newest_pair():
    best, mt = None, 0
    for name in os.listdir(LOGS):
        p = os.path.join(LOGS, name, "Power.log")
        try:
            st = os.stat(p)
        except OSError:
            continue
        if st.st_size > 4096 and st.st_mtime >= mt:
            mt, best = st.st_mtime, p
    if not best:
        return None, None
    zone = os.path.join(os.path.dirname(best), "Zone.log")
    # Zone is a separate logger in log.config: a session logging only
    # Power has no Zone.log at all, and `snapshot()` opened it blind.
    return best, (zone if os.path.exists(zone) else None)


def own_deck_names():
    try:
        from evaluate import try_resolve
        deck, info = try_resolve(MY_DECK)
        names = {cn for cn, _n in (info or {}).get("cards") or []}
        if deck is not None:
            from hs2.optimize import deck_counts
            names |= set(deck_counts(deck))
        return names
    except Exception:
        return set()


def copy_proc_state(zone_path, last_cg, own_names):
    """Opp-copies played from hand, and which hold-this cards were in hand then."""
    seq = 0
    in_hand = {}          # eid -> (seq, name)
    still_hand = {}       # eid -> name currently in hand
    copies_played = []    # (seq, name)
    if not zone_path:     # log.config without the Zone logger
        return copies_played, [], still_hand
    for line in open(zone_path, encoding="utf-8", errors="ignore"):
        try:
            ts = line.split()[1]
        except IndexError:
            continue
        if ts < last_cg:
            continue
        seq += 1
        m = RE_TO_HAND.search(line)
        if m:
            name, eid = m.group(1), int(m.group(2))
            if not name.startswith("UNKNOWN"):
                in_hand.setdefault(eid, (seq, name))
                still_hand[eid] = name
            continue
        m = RE_FROM_HAND.search(line)
        if m:
            name, eid = m.group(1), int(m.group(2))
            still_hand.pop(eid, None)
            if name.startswith("UNKNOWN") or name == "The Coin":
                continue
            if name not in own_names:
                copies_played.append((seq, name))
    armed = []
    for eid, name in still_hand.items():
        entered = in_hand.get(eid, (0, name))[0]
        hits = [n for s, n in copies_played if s > entered]
        if hits:
            armed.append((name, hits[-1]))
    return copies_played, armed, still_hand


def snapshot(power, zone):
    from hs2 import carddata
    if not carddata.DEFS:
        carddata.build_defs()
    last_cg = "00:00:00"
    for line in open(power, encoding="utf-8", errors="ignore"):
        if "GameState.DebugPrintPower() - CREATE_GAME" in line:
            last_cg = line.split()[1]
    ents = {}
    zone_lines = open(zone, encoding="utf-8", errors="ignore") if zone else []
    for line in zone_lines:
        try:
            ts = line.split()[1]
        except IndexError:
            continue
        if ts < last_cg:
            continue
        m = RE_ZONE.search(line)
        if not m:
            continue
        name, eid, zpos, cid, pid, side, dest, extra = m.groups()
        ents[int(eid)] = dict(name=name, cid=cid or "", side=side, zone=dest,
                              extra=extra or "", zpos=int(zpos))
    tags, me = defaultdict(dict), {}
    for line in open(power, encoding="utf-8", errors="ignore"):
        try:
            ts = line.split()[1]
        except IndexError:
            continue
        if ts < last_cg:
            continue
        m = RE_TAG_BR.search(line)
        if m:
            tags[int(m.group(1))][m.group(2)] = m.group(3).rstrip()
            continue
        m = RE_TAG_ID.search(line)
        if m:
            tags[int(m.group(1))][m.group(2)] = m.group(3).rstrip()
            continue
        m = RE_RES.search(line)
        if m:
            me["res"] = int(m.group(1))
        m = RE_USED.search(line)
        if m:
            me["used"] = int(m.group(1))
        m = RE_TURN.search(line)
        if m:
            me["turn"] = int(m.group(1))
        m = RE_STATE.search(line)
        if m:
            me["state"] = m.group(1)
        m = RE_PTAG.search(line)
        if m and m.group(1) != ME:
            me.setdefault("opp", {})[m.group(2)] = m.group(3).rstrip()

    def flag(t, key):
        return t.get(key) in ("1", "True")

    def rows(side, zone_name, extra=""):
        out = []
        for eid, e in ents.items():
            if e["side"] != side or e["zone"] != zone_name or (e["extra"] or "") != extra:
                continue
            t = tags.get(eid, {})
            d = carddata.DEFS.get(e["cid"]) if e["cid"] else None
            cost = t.get("COST", d.cost if d else "?")
            atk = t.get("ATK", d.atk if d else "?")
            hp = t.get("HEALTH", d.hp if d else "?")
            dmg = int(t.get("DAMAGE") or 0)
            try:
                cur = int(hp) - dmg
            except Exception:
                cur = hp
            text = (d.text[:70] if d and d.text else "")
            try:
                ci = int(cost)
            except Exception:
                ci = 99
            try:
                ai = int(atk)
            except Exception:
                ai = 0
            marks = []
            dormant = flag(t, "DORMANT") or "Dormant" in text
            if dormant:
                marks.append("Dormant")
            if (flag(t, "TAUNT") or (d and d.taunt)) and not dormant:
                marks.append("Taunt")
            if flag(t, "DIVINE_SHIELD") or (d and d.divine_shield):
                marks.append("DS")
            if flag(t, "CHARGE") or (d and d.charge):
                marks.append("Charge")
            if flag(t, "RUSH") or (d and d.rush):
                marks.append("Rush")
            if flag(t, "FROZEN"):
                marks.append("Frozen")
            if flag(t, "CANT_ATTACK"):
                marks.append("NoAtk")
            exhausted = flag(t, "EXHAUSTED") or flag(t, "JUST_PLAYED")
            if exhausted and not (flag(t, "CHARGE") or (d and d.charge)):
                marks.append("sick")
            can_atk = (
                ai > 0 and cur != "?" and int(cur) > 0
                and not flag(t, "FROZEN") and not flag(t, "CANT_ATTACK")
                and "sick" not in marks and not dormant
            )
            out.append((e["zpos"], ci, str(cost), e["name"], atk, cur, text,
                        marks, ai, can_atk))
        out.sort()
        return out

    h54, h56 = tags.get(54, {}), tags.get(56, {})
    def hp(t, default):
        try:
            return int(t.get("HEALTH") or default) - int(t.get("DAMAGE") or 0)
        except Exception:
            return "?"

    mine = rows("FRIENDLY", "PLAY")
    opp = rows("OPPOSING", "PLAY")
    hand = rows("FRIENDLY", "HAND")
    hps = rows("FRIENDLY", "PLAY", "Hero Power")
    mana = me.get("res") or 0
    used = me.get("used") or 0
    left = mana - used if isinstance(mana, int) and isinstance(used, int) else "?"

    taunts = [r for r in opp if "Taunt" in r[7] and r[5] != "?" and int(r[5]) > 0]
    ready = [r for r in mine if r[9]]
    face_atk = sum(r[8] for r in ready)

    wipes = []
    for r in hand:
        n = r[3].lower()
        if any(k in n for k in ("medivh", "moonwell", "equality", "ruin",
                                "twisting nether", "brawl")):
            wipes.append(r[3])

    print(f"TURN {me.get('turn','?')} | mana {mana}/{used} used leftover={left}"
          f" | HP {hp(h54,40)} vs {hp(h56,30)} | {me.get('state','')}")
    print("HERO POWER:")
    if hps:
        for r in hps:
            print(f"  [{r[2]}] {r[3]}  {r[6]}")
    else:
        print("  [2] Lesser Heal / Imbue (always spend leftover 2)")
    print("BOARD ME:")
    for r in mine:
        mk = (" " + " ".join(r[7])) if r[7] else ""
        ready_s = " READY" if r[9] else ""
        print(f"  [{r[2]}] {r[3]} {r[4]}/{r[5]}{mk}{ready_s}")
    print("BOARD OPP:")
    for r in opp:
        mk = (" " + " ".join(r[7])) if r[7] else ""
        print(f"  [{r[2]}] {r[3]} {r[4]}/{r[5]}{mk}  {r[6]}")
    print("HAND:")
    for r in hand:
        print(f"  [{r[2]}] {r[3]} {r[4]}/{r[5]}  {r[6]}")

    own = own_deck_names()
    copies, armed, _still = copy_proc_state(zone, last_cg, own)
    hold_cards = []
    for r in hand:
        d = None
        try:
            d = carddata.get_def(r[3])
        except Exception:
            pass
        txt = (d.text or "") if d else r[6]
        if "while holding this" in txt.lower() or "поки тримаєш" in txt.lower():
            hold_cards.append(r[3])
    print("OPP-COPIES PLAYED FROM HAND: " +
          (", ".join(n for _, n in copies) or "none"))
    if hold_cards:
        armed_names = {n for n, _ in armed}
        for n in hold_cards:
            on = n in armed_names
            why = ""
            if on:
                trig = next(t for m, t in armed if m == n)
                why = f" (trigger: played {trig} while holding)"
            print(f"COPY-PROC {n}: {'ON' if on else 'OFF'}{why}")
        print("  Sweeper AOE / Unshackle (1) only if ON — never guess from board")
    if taunts:
        print("TAUNT OPP: " + ", ".join(f"{r[3]} {r[4]}/{r[5]}" for r in taunts))
        print(f"ATTACK: {face_atk} into TAUNT (не в героя, поки таунт живий)")
    else:
        print("TAUNT OPP: none")
        print(f"ATTACK: {face_atk} FACE — бити героя картами зі столу ДО спелів/вайпу")
    if wipes:
        print("WIPE IN HAND: " + ", ".join(wipes) + " — спочатку атаки, потім вайп")
    print("HP RULE: якщо після лінії лишається >=2 мани — обов'язково сила героя")

    opp_hp = hp(h56, 30)
    burn = 0
    burn_n = []
    for r in hand:
        if r[3] in FACE_BURN:
            burn += FACE_BURN[r[3]]
            burn_n.append(f"{r[3]} +{FACE_BURN[r[3]]}")
    face_total = (face_atk if not taunts else 0) + burn
    try:
        need = int(opp_hp)
    except Exception:
        need = 99
    if not taunts and face_atk:
        print(f"PLAN ATTACK: " +
              ", ".join(f"{r[3]} {r[8]}" for r in ready) +
              f" → FACE ({face_atk})")
    elif taunts and ready:
        print("PLAN ATTACK: " +
              ", ".join(f"{r[3]} {r[8]}" for r in ready) +
              " → TAUNT " + taunts[0][3])
    else:
        print("PLAN ATTACK: nothing ready (summoning sickness / empty)")
    if burn_n:
        print("FACE BURN IN HAND: " + ", ".join(burn_n) +
              " (спели в лице ігнорують таунт)")
    if face_total >= need:
        print(f"*** LETHAL? {face_total} >= {need} HP — перевірити порядок атак/спелів ***")

    heroes = rows("OPPOSING", "PLAY", "Hero")
    opp_cls = None
    if heroes:
        d = None
        hn = heroes[0][3]
        try:
            d = carddata.get_def(hn)
        except Exception:
            d = next((x for x in carddata.DEFS.values() if x.name == hn), None)
        if d is not None:
            opp_cls = d.cls
            print(f"OPP HERO: {hn} [{opp_cls}]")
    seen = []
    for e in ents.values():
        if e["side"] != "OPPOSING":
            continue
        if e["name"].startswith("UNKNOWN") or e["name"] in (
                "The Coin", "The Coin (TIME)"):
            continue
        if e["extra"] in ("Hero", "Hero Power"):
            continue
        if e["name"] not in seen:
            seen.append(e["name"])
    try:
        from hs2 import decks
        from hs2.optimize import deck_counts
        metas = [d for d in decks.load_meta()
                 if not opp_cls or d.cls == opp_cls]
        best = None
        for md in metas:
            counts = deck_counts(md)
            hit = sum(1 for s in seen if s in counts)
            frac = hit / len(seen) if seen else 0
            if best is None or hit > best[1] or (hit == best[1] and frac > best[0]):
                best = (frac, hit, md, counts)
        if best and (best[1] >= 2 or (best[0] >= 0.4 and seen)):
            frac, hit, md, counts = best
            print(f"PRED DECK: {md.name} ({hit}/{len(seen)} {frac:.0%})")
            try:
                ores = int(me.get("opp", {}).get("RESOURCES") or 0)
            except Exception:
                ores = 0
            nxt = min(10, ores + 1) if ores < 10 else 10
            threats = []
            for cn, n in counts.items():
                if cn in seen:
                    continue
                try:
                    cd = carddata.get_def(cn)
                except Exception:
                    continue
                if cd.cost <= nxt + 1:
                    threats.append((cd.cost, cn, (cd.text or "")[:55]))
            threats.sort(reverse=True)
            print(f"PLAY AROUND (cost <= {nxt}+1, not seen yet):")
            for c, cn, tx in threats[:6]:
                print(f"  ({c}) {cn} — {tx}")
        elif seen:
            print("PRED DECK: uncertain; seen " + ", ".join(seen[:8]))
    except Exception as ex:
        print(f"PRED DECK: skip ({ex})")

    going_second = any(r[3] == "The Coin" for r in hand)
    if not mine and hand and me.get("turn") in (None, 0, 1) and used == 0:
        print("MULLIGAN HINTS:")
        try:
            import advisor
            data = advisor.load_stats(MY_DECK) if MY_DECK else None
        except Exception:
            data = None
        for r in hand:
            name = r[3]
            if name == "The Coin":
                continue
            try:
                cost = int(r[2])
            except Exception:
                cost = 99
            keep = cost <= 3
            why = "curve"
            if not going_second and cost >= 6:
                keep = False
                why = "too expensive on the play"
            print(f"  {'ЛИШИТИ' if keep else 'СКИНУТИ'} {name} ({cost}) — {why}")
        print("  (Medivh/Atiesh/Karazhan на коїні першого — завжди скид)")

    print("END_SNAPSHOT")


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "wait-next"
    power, zone = newest_pair()
    if not power:
        print("NO_LOG")
        return 1
    if mode == "now":
        snapshot(power, zone)
        return 0
    # Tail Power + Zone: mulligan hand-layout OR our next turn.
    def open_pair(p, z):
        pf = open(p, encoding="utf-8", errors="ignore")
        pf.seek(0, os.SEEK_END)
        zf = None
        if z and os.path.exists(z):
            zf = open(z, encoding="utf-8", errors="ignore")
            zf.seek(0, os.SEEK_END)
        return pf, zf

    pf, zf = open_pair(power, zone)
    seen_off = False
    want_mull = False
    while True:
        np, nz = newest_pair()
        if np and os.path.abspath(np) != os.path.abspath(power):
            pf.close()
            if zf:
                zf.close()
            power, zone = np, nz
            pf, zf = open_pair(power, zone)
            seen_off = False
            want_mull = False
        got = False
        line = pf.readline()
        if line:
            got = True
            if "CREATE_GAME" in line:
                seen_off = True
                want_mull = True
            m = RE_STATE.search(line)
            if m and m.group(1) in ("WON", "LOST", "CONCEDED"):
                print(f"GAME_OVER {m.group(1)}")
                return 0
            m = RE_CUR.search(line)
            if m:
                if m.group(1) == "0":
                    seen_off = True
                elif m.group(1) == "1" and seen_off:
                    time.sleep(0.5)
                    print("EVENT TURN_START")
                    snapshot(power, zone)
                    return 0
        if zf is None and zone and os.path.exists(zone):
            zf = open(zone, encoding="utf-8", errors="ignore")
        if zf:
            zline = zf.readline()
            if zline:
                got = True
                if want_mull and RE_HAND_LAYOUT.search(zline):
                    time.sleep(0.3)
                    print("EVENT MULLIGAN")
                    snapshot(power, zone)
                    return 0
        if not got:
            time.sleep(0.2)


if __name__ == "__main__":
    console.init()
    raise SystemExit(main())
