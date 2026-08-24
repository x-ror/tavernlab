#!/usr/bin/env python3
"""Одноразове завантаження ілюстрацій у локальний кеш.

Чому окремим скриптом, а не запитом з інтерфейсу:

TavernLab обіцяє, що під час гри нічого нікуди не йде. Якщо тягнути
арт карти в момент, коли ви її розглядаєте, CDN дізнається, у що саме
ви граєте — це рівно той витік, якого весь інший код уникає. Тож
завантаження відбувається один раз, свідомо, вашою рукою; далі
`app.py` віддає файли з диска й **ніколи** не ходить у мережу.

    python3 scripts/fetch_art.py            # герої + плитки колекційних карт
    python3 scripts/fetch_art.py --heroes   # лише 11 портретів (~600 КБ)
    python3 scripts/fetch_art.py --art      # ще й повний арт (важче)

Кеш лежить у `art_cache/` і не комітиться: ілюстрації — власність
Blizzard, ми їх кешуємо для особистого використання, а не поширюємо.
"""
import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, REPO)

CACHE = os.path.join(REPO, "art_cache")
BASE = "https://art.hearthstonejson.com/v1"
UA = "TavernLab/0.1 (local personal cache)"

# The 11 classic hero portraits. `hs2/standard_cards.json` carries every
# alternate skin too, but a UI that picks a random skin per session is a
# UI that looks broken, so the mapping is pinned.
HEROES = {
    "WARRIOR": "HERO_01", "SHAMAN": "HERO_02", "ROGUE": "HERO_03",
    "PALADIN": "HERO_04", "HUNTER": "HERO_05", "DRUID": "HERO_06",
    "WARLOCK": "HERO_07", "MAGE": "HERO_08", "PRIEST": "HERO_09",
    "DEMONHUNTER": "HERO_10", "DEATHKNIGHT": "HERO_11",
}

KINDS = {
    "hero": (f"{BASE}/512x/{{}}.jpg", ".jpg"),
    "tile": (f"{BASE}/tiles/{{}}.png", ".png"),
    "art": (f"{BASE}/256x/{{}}.jpg", ".jpg"),
}


def dest(kind, name):
    return os.path.join(CACHE, kind, name + KINDS[kind][1])


def fetch(kind, card_id, name=None):
    """Download one asset unless it is already cached. Returns a status."""
    path = dest(kind, name or card_id)
    if os.path.exists(path) and os.path.getsize(path) > 0:
        return "skip"
    url = KINDS[kind][0].format(card_id)
    try:
        req = urllib.request.Request(url, headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=30) as resp:
            blob = resp.read()
    except urllib.error.HTTPError as exc:
        return "missing" if exc.code == 404 else f"error {exc.code}"
    except Exception as exc:                       # network, TLS, DNS
        return f"error {exc}"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    # Write through a temp name: a half-written file that survives Ctrl-C
    # would be cached forever and render as a broken image.
    tmp = path + ".part"
    with open(tmp, "wb") as fh:
        fh.write(blob)
    os.replace(tmp, path)
    return "ok"


def collectible_ids():
    path = os.path.join(REPO, "hs2", "standard_cards.json")
    with open(path, encoding="utf-8") as fh:
        cards = json.load(fh)
    return sorted(cid for cid, c in cards.items()
                  if c.get("coll") and c.get("type") != "HERO")


def run(jobs, label):
    done = {"ok": 0, "skip": 0, "missing": 0, "error": 0}
    total = len(jobs)
    with ThreadPoolExecutor(max_workers=6) as pool:
        for i, status in enumerate(pool.map(lambda j: fetch(*j), jobs), 1):
            key = status if status in done else "error"
            done[key] += 1
            if i % 50 == 0 or i == total:
                print(f"  {label}: {i}/{total} "
                      f"(нових {done['ok']}, було {done['skip']}, "
                      f"немає {done['missing']}, помилок {done['error']})")
    return done


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--heroes", action="store_true",
                    help="лише портрети героїв")
    ap.add_argument("--art", action="store_true",
                    help="додати повний арт карт (256x), а не лише плитки")
    args = ap.parse_args()

    print(f"Кеш: {CACHE}")
    run([("hero", cid, cls) for cls, cid in sorted(HEROES.items())],
        "герої")
    if args.heroes:
        return 0

    ids = collectible_ids()
    print(f"Колекційних карт: {len(ids)}")
    run([("tile", cid) for cid in ids], "плитки")
    if args.art:
        run([("art", cid) for cid in ids], "арт")

    print("Готово. Інтерфейс підхопить кеш без перезапуску.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
