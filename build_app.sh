#!/bin/bash
# Збірка Таверна-Лаб в один виконуваний файл.
#   Linux/Mac:  ./build_app.sh
#   Windows:    той самий рядок, роздільник даних ";"
#
# Ship target is CPython 3.11 (design A7): the repo compiles under 3.14
# locally, but the onefile must be frozen on 3.11 and smoke-tested on a
# real Windows 11 box — not under Wine (design A14 / PR 17).
#
# Keep this list identical to TavernLab.spec `datas`.
set -e
SEP=":"; [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]] && SEP=";"
pyinstaller --onefile --name TavernLab \
  --add-data "webui.html${SEP}." \
  --add-data "hs2/standard_cards.json${SEP}hs2" \
  --add-data "hs2/meta_decks_2026.json${SEP}hs2" \
  --add-data "hs2/winprob.json${SEP}hs2" \
  --add-data "store/schema.sql${SEP}store" \
  --add-data "locales/en.json${SEP}locales" \
  --add-data "locales/uk.json${SEP}locales" \
  --hidden-import advisor --hidden-import evaluate \
  --hidden-import capture --hidden-import store --hidden-import eval \
  --collect-submodules capture --collect-submodules store \
  --collect-submodules eval \
  --collect-all hslog --collect-all hearthstone \
  app.py
echo "Готово: dist/TavernLab"
