#!/usr/bin/env python3
"""Оновлення мета-гаунтлета новими колодами з трекерів.

  python3 update_meta.py --add "Назва колоди" "AAECA..."     # додати/замінити
  python3 update_meta.py --file new_meta.txt                 # пакетно
  python3 update_meta.py --check                             # покриття карт
  python3 update_meta.py --drop "Назва колоди"

Формат файла: рядки "Назва колоди | AAECA..." (порожні і # ігноруються).
Після зміни гаунтлета перерахуйте матриці: python3 run2_final.py
"""
import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.chdir(os.path.dirname(os.path.abspath(__file__)))

from hs2 import carddata
from hs2.deckstring import decode

META_PATH = "hs2/meta_decks_2026.json"


def load():
    return json.load(open(META_PATH))


def save(data):
    json.dump(data, open(META_PATH, "w"), ensure_ascii=False, indent=1)


def decode_to_entry(code):
    if not carddata.DEFS:
        carddata.build_defs()
    by_dbf = {d.dbf: d for d in carddata.DEFS.values()}
    info = decode(code)
    hero = by_dbf.get(info["heroes"][0]) if info["heroes"] else None
    cards, sb, unknown, unimpl = [], [], [], []
    votes = {}
    for dbf, n in info["cards"]:
        d = by_dbf.get(dbf)
        if d is None:
            unknown.append(dbf)
            continue
        cards.append([d.name, n])
        if d.cls != "NEUTRAL":
            votes[d.cls] = votes.get(d.cls, 0) + n
        if not d.implemented:
            unimpl.append(d.name)
    for dbf, n, owner in info["sideboards"]:
        d = by_dbf.get(dbf)
        if d is None:
            unknown.append(dbf)
            continue
        sb.append([d.name, n])
        if not d.implemented:
            unimpl.append(d.name)
    cls = hero.cls if hero is not None and hero.cls != "NEUTRAL" else \
        (max(votes, key=votes.get) if votes else "NEUTRAL")
    return {"class": cls, "cards": cards, "sideboard": sb,
            "total": sum(n for _, n in cards)}, unknown, sorted(set(unimpl))


def add_deck(data, name, code):
    entry, unknown, unimpl = decode_to_entry(code)
    if unknown:
        print(f"  ✗ {name}: невідомі dbf {unknown} — оновіть датасет "
              "(python3 hs2/build_data.py cards.json)")
        return False
    if unimpl:
        print(f"  ⚠ {name}: НЕРЕАЛІЗОВАНІ карти ({len(unimpl)}):")
        for cn in unimpl:
            d = carddata.get_def(cn)
            print(f"      - {cn}: {d.text[:80]}")
        print("    Колоду додано, але симуляції з нею впадуть, поки "
              "карти не реалізовано в hs2/impls.py.")
    data[name] = entry
    print(f"  ✓ {name} [{entry['class']}] {entry['total']} карт")
    return True


def check(data):
    if not carddata.DEFS:
        carddata.build_defs()
    total_missing = {}
    for name, info in data.items():
        for cn, n in info["cards"] + info.get("sideboard", []):
            try:
                d = carddata.get_def(cn)
            except KeyError:
                total_missing.setdefault(cn, []).append(name)
                continue
            if not d.implemented:
                total_missing.setdefault(cn, []).append(name)
    if not total_missing:
        print(f"Усі карти всіх {len(data)} колод гаунтлета реалізовані ✓")
    else:
        print(f"Нереалізовано {len(total_missing)} карт:")
        for cn, dl in sorted(total_missing.items()):
            print(f"  - {cn}  ({', '.join(dl)})")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--add", nargs=2, metavar=("NAME", "CODE"))
    ap.add_argument("--file")
    ap.add_argument("--drop")
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    data = load()
    changed = False
    if args.add:
        changed |= add_deck(data, args.add[0], args.add[1])
    if args.file:
        for line in open(args.file):
            line = line.strip()
            if not line or line.startswith("#") or "|" not in line:
                continue
            name, code = [x.strip() for x in line.split("|", 1)]
            changed |= add_deck(data, name, code)
    if args.drop:
        if data.pop(args.drop, None) is not None:
            print(f"  − {args.drop} видалено")
            changed = True
    if changed:
        save(data)
        print(f"Гаунтлет: {len(data)} колод -> {META_PATH}")
        print("Не забудьте перерахувати: python3 run2_final.py")
    if args.check or not (args.add or args.file or args.drop):
        check(data)


if __name__ == "__main__":
    main()
