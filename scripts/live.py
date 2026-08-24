#!/usr/bin/env python3
"""Live-помічник: читає Power.log + Zone.log ПІД ЧАС гри і дає поради.

  python3 live.py --logs-dir "<Hearthstone/Logs>" --deck "<деккод>"
  python3 live.py --log Power.log --once

Увімкніть логування: %LOCALAPPDATA%/Blizzard/Hearthstone/log.config
(Wine: users/<user>/AppData/Local/Blizzard/Hearthstone/log.config)
і перезапустіть клієнт.
"""
import argparse
import json
import os
import re
import sys
import time
from types import SimpleNamespace

try:
    sys.stdout.reconfigure(line_buffering=True)
except Exception:
    pass

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import console
import paths
from hs2 import carddata, decks

RE_CREATE = re.compile(r"CREATE_GAME")
RE_SHOW_SIMPLE = re.compile(
    r"SHOW_ENTITY - Updating Entity=(\d+) CardID=(\w+)")
RE_SHOW_BRACKET = re.compile(
    r"SHOW_ENTITY - Updating Entity=\[.*?id=(\d+).*?player=(\d+)\] "
    r"CardID=(\w+)")
RE_TURN = re.compile(r"tag=TURN value=(\d+)")
RE_STATE = re.compile(
    r"TAG_CHANGE Entity=(.+?) tag=PLAYSTATE value=(WON|LOST|TIED)")
RE_ZONE_MOVE = re.compile(
    r"\[entityName=(.+?) id=(\d+) zone=\w+ zonePos=\d+ cardId=(\S*) "
    r"player=(\d+)\] zone from .* -> (FRIENDLY|OPPOSING) "
    r"([A-Z]+)(?: \(([^)]+)\))?")
RE_HAND_LAYOUT = re.compile(
    r"END waiting for zone FRIENDLY HAND")

# Real CREATE_GAME dumps are hundreds of KB. Tiny Power.log files are just
# PowerProcessor error lines written even when verbose logging is off.
MIN_POWER_BYTES = 4096


def lookup(cid="", name=""):
    if cid:
        d = carddata.DEFS.get(cid)
        if d is not None:
            return d
    if name and not name.startswith("UNKNOWN"):
        try:
            return carddata.get_def(name)
        except KeyError:
            low = name.lower()
            for d in carddata.DEFS.values():
                if d.name.lower() == low:
                    return d
    if name and not name.startswith("UNKNOWN"):
        return SimpleNamespace(name=name, cost="?", cls="NEUTRAL",
                               type="UNKNOWN", coll=False)
    return None


class GameTracker:
    def __init__(self, my_name=None):
        self.my_name = my_name
        self.deck_names = set()
        self.reset()

    def reset(self):
        forced = getattr(self, "forced_pid", None)
        self.players = {}
        self.classes = {}
        self.played = {1: [], 2: []}
        self.turn = 0
        self.result = None
        self.entity_cards = {}
        self.my_pid = None
        self.mulligan_shown = False
        self.my_start = []
        self.seen_plays = set()
        self.pid_score = {1: 0, 2: 0}


def process_power(tr, line, on_event):
    if RE_CREATE.search(line):
        if tr.played[1] or tr.played[2]:
            on_event("game_reset", None)
        tr.reset()
        return
    m = RE_SHOW_SIMPLE.search(line)
    if m:
        tr.entity_cards[int(m.group(1))] = m.group(2)
        return
    m = RE_SHOW_BRACKET.search(line)
    if m:
        tr.entity_cards[int(m.group(1))] = m.group(3)
        return
    m = RE_TURN.search(line)
    if m:
        tr.turn = int(m.group(1))
        return
    m = RE_STATE.search(line)
    if m and tr.result is None and m.group(2) in ("WON", "LOST"):
        winner = m.group(1).strip() if m.group(2) == "WON" else None
        tr.result = {"winner": winner}
        on_event("game_end", None)


def process_zone(tr, line, on_event):
    if RE_HAND_LAYOUT.search(line) and not tr.mulligan_shown and tr.my_start:
        tr.mulligan_shown = True
        on_event("mulligan", None)
        return
    m = RE_ZONE_MOVE.search(line)
    if not m:
        return
    name, eid, cid, pid, side, zone, extra = (
        m.group(1), int(m.group(2)), m.group(3) or "",
        int(m.group(4)), m.group(5), m.group(6), m.group(7))
    if cid:
        tr.entity_cards[eid] = cid
    if side == "FRIENDLY":
        tr.my_pid = pid
    d = lookup(cid, name)

    if zone == "PLAY" and extra == "Hero" and d is not None:
        if d.cls and d.cls != "NEUTRAL":
            tr.classes[pid] = d.cls
            who = "ви" if side == "FRIENDLY" else "суперник"
            print(f"  {who}: {d.cls.title()} ({d.name})", flush=True)
        return

    if zone == "HAND" and side == "FRIENDLY" and not tr.mulligan_shown:
        if name.startswith("UNKNOWN") or name == "The Coin":
            return
        if extra:  # Hero / Hero Power
            return
        if name not in tr.my_start:
            tr.my_start.append(name)
        return

    if zone == "PLAY" and extra is None and eid not in tr.seen_plays:
        if d is None or d.type in ("HERO", "HERO_POWER", "ENCHANTMENT"):
            return
        tr.seen_plays.add(eid)
        tr.played.setdefault(pid, []).append(d.name)
        if d.cls and d.cls != "NEUTRAL" and pid not in tr.classes:
            tr.classes[pid] = d.cls
        on_event("play", (pid, d))


def find_power_log(logs_dir, since_mtime=0):
    """Newest Power.log that looks like real verbose logging."""
    best, best_mtime = None, since_mtime
    try:
        names = os.listdir(logs_dir)
    except OSError:
        return None
    for name in names:
        cand = os.path.join(logs_dir, name, "Power.log")
        try:
            st = os.stat(cand)
        except OSError:
            continue
        if st.st_size < MIN_POWER_BYTES or st.st_mtime < since_mtime:
            continue
        if st.st_mtime >= best_mtime:
            best_mtime = st.st_mtime
            best = cand
    return best


def advise_predict(tr, my_pid):
    from hs2.optimize import deck_counts
    opp_pid = 2 if my_pid == 1 else 1
    seen = tr.played.get(opp_pid, [])
    cls = tr.classes.get(opp_pid)
    if not cls:
        return
    opps = [d for d in decks.load_meta() if d.cls == cls]
    for opp in opps:
        counts = deck_counts(opp)
        hit = sum(1 for s in set(seen) if s in counts)
        frac = hit / max(1, len(set(seen)))
        if frac >= 0.5:
            unseen = sorted([(cn, carddata.get_def(cn).cost)
                             for cn in counts if cn not in seen],
                            key=lambda x: -x[1])[:5]
            print(f"  ▸ схоже на «{opp.name}» ({frac:.0%}). Чекай: " +
                  ", ".join(f"({c}) {n}" for n, c in unseen), flush=True)


def advise_mulligan(tr, deck_code):
    if not tr.my_start:
        return
    opp_pid = 2 if (tr.my_pid or 1) == 1 else 1
    opp_cls = tr.classes.get(opp_pid)
    print(f"  Муліган ({len(tr.my_start)} карт"
          + (f", vs {opp_cls.title()}" if opp_cls else "") + "):",
          flush=True)
    data = None
    if deck_code:
        try:
            import advisor
            data = advisor.load_stats(deck_code)
        except SystemExit:
            data = None
    opps = [d for d in decks.load_meta()
            if not opp_cls or d.cls == opp_cls]
    for name in tr.my_start[:5]:
        try:
            import advisor
            card = advisor.find_card(name)
        except (SystemExit, Exception):
            card = lookup("", name)
        if card is None:
            print(f"    ? {name}", flush=True)
            continue
        delta, ns = None, 0
        if data:
            deltas = []
            for opp in opps:
                s = data["stats"].get(opp.name)
                if not s:
                    continue
                base = s["wins"] / s["games"]
                rec = s["cards"].get(card.name)
                if rec and rec[0] >= 30:
                    deltas.append(rec[1] / rec[0] - base)
                    ns += rec[0]
            if deltas:
                delta = sum(deltas) / len(deltas)
        keep = (delta > -0.01) if delta is not None else (
            getattr(card, "cost", 99) != "?" and card.cost <= 3)
        d_txt = f" ({delta:+.1%})" if delta is not None else " (мало даних)"
        print(f"    {'ЛИШИТИ ' if keep else 'СКИНУТИ'} {card.name}{d_txt}",
              flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", default=None)
    ap.add_argument("--logs-dir", default=None,
                    help="папка Logs гри: чекає найновішу сесію з Power.log")
    ap.add_argument("--deck", default="", help="ваш деккод (для мулігану)")
    ap.add_argument("--name", default="", help="ваш нік у грі")
    ap.add_argument("--me", type=int, default=0,
                    help="примусово: ваш PlayerID (1 або 2)")
    ap.add_argument("--once", action="store_true",
                    help="обробити файл разово, без стеження")
    ap.add_argument("--out", default=paths.in_home("games.jsonl"))
    args = ap.parse_args()
    carddata.build_defs()
    log_path = args.log
    since = time.time() - 300
    if log_path is None and args.logs_dir:
        print("Чекаю на Power.log нової сесії…", flush=True)
        while True:
            log_path = find_power_log(args.logs_dir, since_mtime=since)
            if log_path:
                break
            time.sleep(2)
        print(f"Знайшов: {log_path}", flush=True)
    if not log_path:
        ap.error("вкажіть --log або --logs-dir")
    args.log = log_path
    tr = GameTracker(my_name=args.name or None)
    if args.me:
        tr.my_pid = args.me
        tr.forced_pid = args.me
    if args.deck:
        try:
            from evaluate import try_resolve
            _d, _info = try_resolve(args.deck)
            tr.deck_names = {cn for cn, _n in _info.get("cards", [])}
        except Exception:
            pass

    def on_event(kind, payload):
        my_pid = tr.my_pid or 1
        if kind == "mulligan":
            advise_mulligan(tr, args.deck)
        elif kind == "play":
            pid, d = payload
            side = "ви" if pid == my_pid else "суперник"
            print(f"[хід {tr.turn}] {side}: {d.name} ({d.cost})", flush=True)
            if pid != my_pid:
                advise_predict(tr, my_pid)
        elif kind == "game_end":
            rec = {"ts": time.time(), "players": tr.players,
                   "classes": tr.classes, "played": tr.played,
                   "result": tr.result, "turns": tr.turn}
            with open(args.out, "a", encoding="utf-8") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            print(f"■ Гра завершена ({tr.result}). Записано в {args.out}",
                  flush=True)

    def open_pair(power_path):
        zone_path = os.path.join(os.path.dirname(power_path), "Zone.log")
        pf = open(power_path, encoding="utf-8", errors="ignore")
        zf = None
        if os.path.exists(zone_path):
            zf = open(zone_path, encoding="utf-8", errors="ignore")
        return pf, zf, zone_path

    def follow(path):
        pf, zf, zone_path = open_pair(path)
        for line in pf:
            process_power(tr, line, on_event)
        if zf:
            for line in zf:
                process_zone(tr, line, on_event)
        if args.once:
            pf.close()
            if zf:
                zf.close()
            return
        print("— стежу за логом (Ctrl+C для виходу) —", flush=True)
        while True:
            if args.logs_dir:
                newer = find_power_log(args.logs_dir, since_mtime=since)
                if newer and os.path.abspath(newer) != os.path.abspath(path):
                    print(f"Нова сесія: {newer}", flush=True)
                    pf.close()
                    if zf:
                        zf.close()
                    tr.reset()
                    return newer
            got = False
            line = pf.readline()
            if line:
                process_power(tr, line, on_event)
                got = True
            if zf is None and os.path.exists(zone_path):
                zf = open(zone_path, encoding="utf-8", errors="ignore")
            if zf:
                line = zf.readline()
                if line:
                    process_zone(tr, line, on_event)
                    got = True
            if not got:
                time.sleep(0.25)

    try:
        while log_path:
            log_path = follow(log_path)
            if args.once:
                break
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    console.init()
    main()


# ================================================== board-state tracking
RE_TAG_NUM = re.compile(
    r"TAG_CHANGE Entity=(\d+) tag=(\w+) value=(\S+)")
RE_TAG_BRACKET = re.compile(
    r"TAG_CHANGE Entity=\[.*?id=(\d+).*?\] tag=(\w+) value=(\S+)")
RE_INDENT_TAG = re.compile(r"^\s+tag=(\w+) value=(\S+)")
RE_FULL_CREATE = re.compile(r"FULL_ENTITY - Creating ID=(\d+) CardID=(\w*)")
RE_SHOW_NUM = re.compile(r"SHOW_ENTITY - Updating Entity=(\d+) CardID=(\w+)")
RE_SHOW_BR = re.compile(
    r"SHOW_ENTITY - Updating Entity=\[.*?id=(\d+).*?\] CardID=(\w+)")
RE_PLAYER_ENT = re.compile(r"Player EntityID=(\d+) PlayerID=(\d+)")

TRACK_TAGS = {"ZONE", "CONTROLLER", "ATK", "HEALTH", "DAMAGE", "COST",
              "ARMOR", "RESOURCES", "RESOURCES_USED", "TEMP_RESOURCES",
              "CURRENT_PLAYER", "CARDTYPE", "TAUNT", "DIVINE_SHIELD",
              "FROZEN", "STEALTH", "EXHAUSTED", "NUM_ATTACKS_THIS_TURN",
              "WINDFURY", "DORMANT"}


class BoardState:
    def __init__(self):
        self.ents = {}          # id -> tags dict (+ "card_id")
        self.player_ents = {}   # player_id -> entity id
        self.cur_ent = None

    def ent(self, eid):
        return self.ents.setdefault(eid, {})

    def feed(self, line):
        m = RE_PLAYER_ENT.search(line)
        if m:
            self.player_ents[int(m.group(2))] = int(m.group(1))
            self.cur_ent = int(m.group(1))
            return
        m = RE_FULL_CREATE.search(line)
        if m:
            self.cur_ent = int(m.group(1))
            if m.group(2):
                self.ent(self.cur_ent)["card_id"] = m.group(2)
            return
        m = RE_SHOW_NUM.search(line) or RE_SHOW_BR.search(line)
        if m:
            self.cur_ent = int(m.group(1))
            self.ent(self.cur_ent)["card_id"] = m.group(2)
            return
        m = RE_TAG_NUM.search(line) or RE_TAG_BRACKET.search(line)
        if m:
            eid, tag, val = int(m.group(1)), m.group(2), m.group(3)
            if tag in TRACK_TAGS:
                self.ent(eid)[tag] = val
            return
        m = RE_INDENT_TAG.match(line)
        if m and self.cur_ent is not None:
            tag, val = m.group(1), m.group(2)
            if tag in TRACK_TAGS:
                self.ent(self.cur_ent)[tag] = val

    def _int(self, e, tag, default=0):
        try:
            return int(e.get(tag, default))
        except (ValueError, TypeError):
            return default

    def side(self, pid, zone):
        out = []
        for eid, e in self.ents.items():
            if e.get("ZONE") == zone and \
                    self._int(e, "CONTROLLER") == pid and \
                    e.get("card_id"):
                out.append((eid, e))
        return out

    def mana(self, pid):
        pe = self.ents.get(self.player_ents.get(pid, -1), {})
        return (self._int(pe, "RESOURCES") +
                self._int(pe, "TEMP_RESOURCES") -
                self._int(pe, "RESOURCES_USED"))

    def hero_hp(self, pid):
        for eid, e in self.ents.items():
            if e.get("CARDTYPE") == "HERO" and \
                    self._int(e, "CONTROLLER") == pid and \
                    e.get("ZONE") == "PLAY":
                return (self._int(e, "HEALTH", 30) -
                        self._int(e, "DAMAGE") + self._int(e, "ARMOR"))
        return 30


def build_sim_state(bs, my_pid, my_deck_code):
    """QUARANTINED — superseded by `eval.mapper.build_overlay` (PR 8).

    Three things in here are wrong for a real position and cannot be
    patched in place:

    * it calls `Game.start()`, which shuffles, mulligans and fires every
      start-of-game effect on top of the board it is about to overwrite;
    * it hardcodes `g.turn = 10`, so every win-probability and every
      turn-scaled effect reads a turn that never happened;
    * it deals the **user's own deck to both players** (`Game(deck,
      deck, …)`), which invents a 15-card opponent list we have never
      seen and leaks the user's cards into the opponent's pool.

    `eval.mapper.build_overlay` calls `Game.__init__` only, gives the
    opponent implemented fillers of *their* class, takes `turn` from the
    VisibleState, and reports `lethal_ok` / `hand_complete` instead of
    pretending the position is rules-complete.
    """
    raise NotImplementedError(
        "live.build_sim_state is quarantined; use "
        "eval.mapper.build_overlay(visible_state) instead (design PR 8)")


def _legacy_build_sim_state(bs, my_pid, my_deck_code):
    """Kept only so the quarantine note above can be checked against the
    code it describes. Not called from anywhere."""
    from evaluate import try_resolve
    from hs2.engine import Game, Minion, CardInst
    from hs2.ai import Agent
    opp_pid = 2 if my_pid == 1 else 1
    deck, _info = try_resolve(my_deck_code)
    if deck is None:
        return None, None
    g = Game(deck, deck, seed=1, agents=[Agent("midrange"),
                                         Agent("midrange")])
    g.start(first=0)
    me, opp = g.players[0], g.players[1]
    for p in (me, opp):
        p.hand.clear()
        p.board.clear()
        p.deck = p.deck[:15]
    me.hp = min(bs.hero_hp(my_pid), 40)
    me.max_hp = max(me.hp, 30)
    opp.hp = min(bs.hero_hp(opp_pid), 40)
    opp.max_hp = max(opp.hp, 30)
    me.crystals = me.mana = max(0, min(bs.mana(my_pid), 10))
    g.turn = 10
    g.current = 0

    def add_minion(p, e):
        d = carddata.DEFS.get(e.get("card_id"))
        if d is None or d.type != "MINION":
            return
        m = Minion(d, p)
        atk = bs._int(e, "ATK")
        hp = bs._int(e, "HEALTH")
        if atk:
            m.atk_base = atk
        if hp:
            m.hp_base = hp
        m.damage = bs._int(e, "DAMAGE")
        m.taunt = e.get("TAUNT") == "1" or m.taunt
        m.divine_shield = e.get("DIVINE_SHIELD") == "1" or m.divine_shield
        m.frozen = 1 if e.get("FROZEN") == "1" else 0
        m.dormant = 1 if e.get("DORMANT") == "1" else 0
        m.just_summoned = False
        m.attacks_done = bs._int(e, "NUM_ATTACKS_THIS_TURN")
        if e.get("EXHAUSTED") == "1" and m.attacks_done == 0:
            m.just_summoned = True
        p.board.append(m)

    for eid, e in bs.side(my_pid, "PLAY"):
        add_minion(me, e)
    for eid, e in bs.side(opp_pid, "PLAY"):
        add_minion(opp, e)
    for eid, e in bs.side(my_pid, "HAND"):
        d = carddata.DEFS.get(e.get("card_id"))
        if d is not None and d.coll:
            me.hand.append(CardInst(d))
    g.recompute_auras()
    return g, me


def advise_turn(bs, my_pid, deck_code):
    """Never wired into `main()`; the live path is v1 (design PR 6).

    Now routed through the PR 8 overlay so that when PR 6 does wire it up
    there is only one mapper to keep honest.
    """
    try:
        g, me = _overlay_from_boardstate(bs, my_pid, deck_code)
        if g is None:
            return
        from hs2.lethal import find_lethal
        plan = find_lethal(g, me)
        opp = me.opponent
        print(f"  ── ваш хід: мана {me.mana}, ви {me.hp}hp, "
              f"суперник {opp.hp}hp, стіл {len(me.active_minions)}"
              f"×{len(opp.active_minions)}")
        if plan:
            steps = []
            for act in plan:
                if act[0] == "spell":
                    steps.append(f"{act[1].card.name}→лице")
                elif act[0] == "attack":
                    t = act[2]
                    tn = t.card.name if hasattr(t, "card") else "лице"
                    steps.append(f"{act[1].card.name}→{tn}")
                elif act[0] == "hero_power":
                    steps.append("геройська сила")
                elif act[0] == "hero_attack":
                    steps.append("герой→лице")
            print(f"  ☠ ЛЕТАЛ Є: " + ", ".join(steps))
            return
        agent = g.agents[0]
        scored = []
        for inst in me.hand:
            if me.effective_cost(inst) > me.mana:
                continue
            res = agent.evaluate_play(g, me, inst)
            if res and res[0] > 0:
                tgt = res[1]
                tn = ""
                if tgt is not None and hasattr(tgt, "card"):
                    tn = f" → {tgt.card.name}"
                elif tgt is opp:
                    tn = " → лице"
                scored.append((res[0], f"{inst.card.name} "
                               f"({me.effective_cost(inst)}){tn}"))
        scored.sort(reverse=True)
        if scored:
            print("  ► рекомендую: " +
                  ";  ".join(s for _, s in scored[:3]))
        else:
            print("  ► грати нічого — атакуйте і завершуйте хід")
    except Exception as ex:
        print(f"  (порада ходу недоступна: {ex})")


def _overlay_from_boardstate(bs, my_pid, deck_code=None):
    """`BoardState` -> `eval.mapper` overlay.

    `BoardState` already tracks the tags the reconstructor wants, so it is
    promoted into a `VisibleState` rather than re-implemented.  Returns
    `(game, our_player)` or `(None, None)`; `lethal_ok` is enforced here so
    a caller cannot accidentally run lethal on a state that did not parse.
    """
    from eval.mapper import build_overlay
    from eval.types import EntityView, VisibleState

    opp_pid = 2 if my_pid == 1 else 1
    vs = VisibleState(turn=bs.turn or 0, us=my_pid,
                      current_player=my_pid)

    def views(pid, zone):
        out = []
        for eid, e in bs.side(pid, zone):
            tags = {k: v for k, v in e.items()
                    if k not in ("card_id", "ATK", "HEALTH", "DAMAGE",
                                 "COST")}
            tags.setdefault("CARDTYPE", e.get("CARDTYPE", "MINION"))
            out.append(EntityView(
                eid=int(eid), card_id=e.get("card_id") or None,
                controller=pid, zone=zone,
                atk=bs._int(e, "ATK") or None,
                health=bs._int(e, "HEALTH") or None,
                damage=bs._int(e, "DAMAGE"),
                cost=bs._int(e, "COST") or None,
                tags={k: (1 if v == "1" else v) for k, v in tags.items()}))
        return out

    for pid in (my_pid, opp_pid):
        vs.heroes[pid] = {"hp": bs.hero_hp(pid), "armor": 0, "atk": 0,
                          "attacks": 0}
        vs.mana[pid] = {"crystals": max(0, min(bs.mana(pid), 10)),
                        "used": 0}
        vs.boards[pid] = views(pid, "PLAY")
        vs.hands[pid] = views(pid, "HAND") if pid == my_pid else []
        vs.deck_counts[pid] = 0

    ov = build_overlay(vs, us_pid=my_pid)
    if not ov.lethal_ok:
        return None, None
    return ov.game, ov.us
