#!/bin/bash
# Збірка Таверна-Лаб в один виконуваний файл.
#   Linux/Mac:  ./build_app.sh
#   Windows:    той самий рядок, роздільник даних ";"
#
# Ship target is the current CPython (design A7): there is no
# version-specific code here, so freeze on whatever is newest rather
# than maintaining an old interpreter. The onefile still has to be
# smoke-tested on a real Windows 11 box — not under Wine (A14 / PR 17).
#
# Keep this list identical to TavernLab.spec `datas`.
set -e
SEP=":"; [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]] && SEP=";"
# NOTE: this overwrites the committed `TavernLab.spec` with a generated
# one that drops the hand-written hiddenimports — restore it after a run
# (`git checkout -- TavernLab.spec`). `--specpath` is not the fix: it
# re-roots every relative `--add-data` at the spec directory.
pyinstaller --onefile --name TavernLab \
  --add-data "web/dist${SEP}web/dist" \
  --add-data "hs2/standard_cards.json${SEP}hs2" \
  --add-data "hs2/wild_cards.json${SEP}hs2" \
  --add-data "hs2/meta_decks_2026.json${SEP}hs2" \
  --add-data "hs2/wild_decks.json${SEP}hs2" \
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
