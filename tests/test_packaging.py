"""PR 17: the frozen build must ship what the app reads at runtime.

The bug being pinned: `hs2/winprob.json` was in `build_app.sh` but not in
`TavernLab.spec`, so a spec-only freeze served /api/winprob straight into
a FileNotFoundError. These tests read both build definitions and check
them against the files the code actually opens.
"""
import os
import re

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = os.path.join(ROOT, "TavernLab.spec")
SH = os.path.join(ROOT, "build_app.sh")

# Everything opened by path at runtime rather than imported.
RUNTIME_DATA = [
    "webui.html",
    "hs2/standard_cards.json",
    "hs2/meta_decks_2026.json",
    "hs2/winprob.json",
    "store/schema.sql",
    "locales/en.json",
    "locales/uk.json",
]


def spec_datas():
    src = open(SPEC).read()
    return set(re.findall(r"\('([^']+)',\s*'[^']*'\)", src))


def sh_datas():
    src = open(SH).read()
    return set(re.findall(r'--add-data "([^"$]+)\$\{SEP\}', src))


@pytest.mark.parametrize("path", RUNTIME_DATA)
def test_spec_ships_every_runtime_data_file(path):
    assert path in spec_datas(), f"{path} missing from TavernLab.spec"


@pytest.mark.parametrize("path", RUNTIME_DATA)
def test_build_script_ships_every_runtime_data_file(path):
    assert path in sh_datas(), f"{path} missing from build_app.sh"


def test_spec_and_script_agree():
    assert spec_datas() == sh_datas(), (
        "the two build paths would freeze different bundles")


@pytest.mark.parametrize("path", RUNTIME_DATA)
def test_the_data_file_actually_exists(path):
    assert os.path.exists(os.path.join(ROOT, path)), f"{path} not in repo"


def test_lazily_imported_packages_are_hidden_imports():
    """capture/store/eval are imported inside request handlers, so
    PyInstaller's static analysis never sees them."""
    src = open(SPEC).read()
    for mod in ("capture", "store", "eval", "hslog", "advisor",
                "evaluate"):
        assert f"'{mod}'" in src, f"{mod} missing from hiddenimports"


def test_requirements_pin_hslog_exactly():
    req = open(os.path.join(ROOT, "requirements.txt")).read()
    assert re.search(r"^hslog==", req, re.M), "hslog must be pinned =="
    assert re.search(r"^hearthstone==", req, re.M)
    assert "watchdog" not in req.split("# Deliberately absent")[0]
    for banned in ("numpy", "torch"):
        assert banned not in req.split("# Deliberately absent")[0]


def test_tos_grep_is_clean():
    import subprocess
    import sys
    out = subprocess.run([sys.executable, "tools_tos_grep.py"], cwd=ROOT,
                         capture_output=True, text=True)
    assert out.returncode == 0, out.stdout


def test_every_eval_and_capture_module_is_a_hidden_import():
    """These are imported inside request handlers and job threads, so
    PyInstaller's static analysis never sees them — a missing one is a
    frozen-only ImportError nobody hits in development."""
    import os
    src = open(SPEC).read()
    for pkg in ("eval", "capture", "store"):
        for name in sorted(os.listdir(os.path.join(ROOT, pkg))):
            if not name.endswith(".py") or name == "__init__.py":
                continue
            mod = f"{pkg}.{name[:-3]}"
            assert f"'{mod}'" in src, f"{mod} missing from hiddenimports"


def test_bundled_data_is_addressed_relative_to_its_module():
    """In a PyInstaller onefile, `app.py` does `os.chdir(WORKDIR)` while
    the bundled data sits under `sys._MEIPASS`. Anything opened relative
    to the CWD is therefore a frozen-only FileNotFoundError."""
    import os
    import re
    bad = []
    for pkg in ("capture", "store", "eval"):
        for name in sorted(os.listdir(os.path.join(ROOT, pkg))):
            if not name.endswith(".py"):
                continue
            src = open(os.path.join(ROOT, pkg, name)).read()
            for m in re.finditer(r'open\(\s*["\']([^"\']+\.(?:json|sql))',
                                 src):
                bad.append(f"{pkg}/{name}: relative open({m.group(1)!r})")
    assert not bad, bad


def test_the_schema_and_card_corpus_resolve_from_their_modules():
    import os
    from store.db import SCHEMA_PATH
    from eval.visible import _CARDS_PATH
    for path in (SCHEMA_PATH, _CARDS_PATH):
        assert os.path.isabs(path)
        assert os.path.exists(path), path
    # …and they sit where the spec puts them in the bundle.
    assert SCHEMA_PATH.endswith(os.path.join("store", "schema.sql"))
    assert _CARDS_PATH.endswith(
        os.path.join("hs2", "standard_cards.json"))
