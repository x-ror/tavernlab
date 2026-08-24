#!/usr/bin/env python3
"""CLI над імпортером Power.log (PR 5).

Раніше цей скрипт викликав `hslog.export.EntityTreeExporter`, який
повертає *кінцеве* дерево сутностей: карта, яку зіграли й яка померла,
видно лише в останній зоні. Дошки по ходах там немає взагалі, тому
експортер більше не використовується — розбір іде по Packet Tree
(`capture/hslog_import.py`), а результат лягає в SQLite, не в
`games.jsonl`.

Підготовка:
  1) Увімкніть логування (Налаштування → вкладка «Імпорт» покаже
     готовий текст `log.config`).
  2) pip install -r requirements.txt
  3) python3 watcher.py --log "/шлях/до/Logs/Power.log"
     python3 watcher.py --last-session --logs-dir "/шлях/до/Logs"

Далі відкрийте застосунок (`python3 app.py`) — вкладка «Ігри».
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--log", help="шлях до Power.log")
    ap.add_argument("--logs-dir", help="тека Logs (для --last-session)")
    ap.add_argument("--last-session", action="store_true",
                    help="взяти найновішу сесію з --logs-dir")
    ap.add_argument("--only-last", action="store_true",
                    help="імпортувати лише останню гру з файлу")
    ap.add_argument("--workdir", default=os.path.dirname(
        os.path.abspath(__file__)), help="де лежить tavernlab.sqlite")
    ap.add_argument("--dry-run", action="store_true",
                    help="розібрати й показати, нічого не записуючи")
    args = ap.parse_args()

    from capture.hslog_import import (import_log, newest_power_log,
                                      parsed_games)

    path = args.log
    if args.last_session or not path:
        logs_dir = args.logs_dir or os.environ.get("HS_LOGS")
        if not logs_dir:
            raise SystemExit("Вкажіть --log або --logs-dir/HS_LOGS")
        path = newest_power_log(logs_dir)
        if not path:
            raise SystemExit(f"Немає придатного Power.log у {logs_dir}")
        print(f"Найновіша сесія: {path}")

    if args.dry_run:
        for i, (_raw, events, summ, _p) in enumerate(parsed_games(path)):
            print(f"[{i}] подій {len(events)}, ходів {summ.turns}, "
                  f"класи {summ.classes}, переможець "
                  f"{summ.winner_pid}, режим {summ.mode}")
        return

    from store import open_store
    store = open_store(args.workdir)
    try:
        ids = import_log(store, path, only_last=args.only_last)
    finally:
        store.close()
    print(f"Імпортовано ігор: {len(ids)} → "
          f"{os.path.join(args.workdir, 'tavernlab.sqlite')}")
    if not ids:
        print("(нових ігор немає — цей лог уже імпортовано)")
    print("Далі: python3 app.py → вкладка «Ігри».")


if __name__ == "__main__":
    main()
