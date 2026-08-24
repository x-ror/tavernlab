#!/usr/bin/env python3
"""Оновлення мета-гаунтлета новими колодами з трекерів.

  python3 update_meta.py --add "Назва колоди" "AAECA..."     # додати/замінити
  python3 update_meta.py --file new_meta.txt                 # пакетно
  python3 update_meta.py --check                             # покриття карт
  python3 update_meta.py --drop "Назва колоди"

Гаунтлет свій для кожного формату; `--format wild` працює з
`hs2/wild_decks.json`. Це і є шлях замінити згенерований baseline
справжніми колодами з трекерів.

Формат файла: рядки "Назва колоди | AAECA..." (порожні і # ігноруються).
Після зміни гаунтлета перерахуйте матриці: python3 run2_final.py
"""
import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.chdir(os.path.dirname(os.path.abspath(__file__)))

import console
from hs2 import carddata, decks, formats
from hs2.deckstring import decode


def load(fmt):
    path = decks.gauntlet_path(fmt)
    if not os.path.exists(path):
        return {}
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def save(data, fmt):
    with open(decks.gauntlet_path(fmt), "w", encoding="utf-8") as fh:
        json.dump(data, fh, ensure_ascii=False, indent=1)


def decode_to_entry(code, fmt):
    carddata.ensure_defs(fmt)
    by_dbf = {d.dbf: d for d in carddata.DEFS.values()}
    info = decode(code)
    declared = formats.from_deckstring(info.get("format"))
    if declared and declared != fmt:
        print(f"  ⚠ деккод заявляє формат «{declared}», а гаунтлет "
              f"«{fmt}» — перевірте --format")
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


def add_deck(data, name, code, fmt):
    entry, unknown, unimpl = decode_to_entry(code, fmt)
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


def check(data, fmt):
    carddata.ensure_defs(fmt)
    if not data:
        print(f"Гаунтлета для «{fmt}» немає "
              f"({decks.gauntlet_path(fmt)})")
        return
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
    ap.add_argument("--format", choices=formats.FORMATS,
                    default=formats.STANDARD)
    args = ap.parse_args()
    data = load(args.format)
    changed = False
    if args.add:
        changed |= add_deck(data, args.add[0], args.add[1], args.format)
    if args.file:
        for line in open(args.file):
            line = line.strip()
            if not line or line.startswith("#") or "|" not in line:
                continue
            name, code = [x.strip() for x in line.split("|", 1)]
            changed |= add_deck(data, name, code, args.format)
    if args.drop:
        if data.pop(args.drop, None) is not None:
            print(f"  − {args.drop} видалено")
            changed = True
    if changed:
        save(data, args.format)
        print(f"Гаунтлет [{args.format}]: {len(data)} колод -> "
              f"{decks.gauntlet_path(args.format)}")
        print("Не забудьте перерахувати: python3 scripts/run2_final.py")
    if args.check or not (args.add or args.file or args.drop):
        check(data, args.format)


if __name__ == "__main__":
    console.init()
    main()
