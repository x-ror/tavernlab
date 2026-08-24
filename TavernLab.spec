# -*- mode: python ; coding: utf-8 -*-
#
# PR 17. Two things were broken before and are pinned here:
#
#  * `hs2/winprob.json` was in build_app.sh but NOT in this spec, so a
#    spec-only freeze raised FileNotFoundError the first time anyone hit
#    /api/winprob. The two build paths now carry the same datas.
#  * hiddenimports listed only advisor/evaluate. capture, store and eval
#    are imported lazily inside request handlers, so PyInstaller's static
#    analysis never sees them; hslog is imported by name inside
#    capture.hslog_import.

datas = [
    # The React/Spectrum UI — the whole front end since the vanilla
    # `webui.html` was retired. Built by `npm run build` in web/;
    # PyInstaller only copies it, so a freeze made without that step
    # answers 404 on every page.
    ('web/dist', 'web/dist'),
    ('hs2/standard_cards.json', 'hs2'),
    # The Wild pool and its gauntlet. Wild is a build-time choice the way
    # web/dist is: without them the app serves Standard and says so.
    ('hs2/wild_cards.json', 'hs2'),
    ('hs2/meta_decks_2026.json', 'hs2'),   # Standard gauntlet
    ('hs2/wild_decks.json', 'hs2'),        # Wild gauntlet
    ('hs2/winprob.json', 'hs2'),
    ('store/schema.sql', 'store'),
    ('locales/en.json', 'locales'),
    ('locales/uk.json', 'locales'),
]

hiddenimports = [
    'advisor', 'evaluate',
    'capture', 'capture.events', 'capture.hslog_import',
    'store', 'store.db', 'store.migrate_jsonl',
    'eval', 'eval.types', 'eval.visible', 'eval.snapshots',
    'eval.mapper', 'eval.ledger', 'eval.classify', 'eval.review',
    'eval.taggers', 'eval.i18n', 'eval.solvers', 'eval.solvers.lethal',
    'hslog', 'hslog.parser', 'hslog.packets', 'hslog.player',
    'hslog.tokens', 'hslog.export',
    'hearthstone', 'hearthstone.enums', 'aniso8601',
]

a = Analysis(
    ['app.py'],
    pathex=[],
    binaries=[],
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name='TavernLab',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
