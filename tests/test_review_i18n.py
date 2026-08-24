"""The review must be storable once and readable in either language.

A review is written into `reviews.summary` and read back for years, so
it cannot be frozen in whatever language was selected the day it ran.
`eval/i18n.py` therefore emits every message as a stable key plus its
numbers, and renders the English from `locales/en.json` so the stored
fallback cannot drift away from the translation.

Two things have to hold, and neither is visible by reading one file:

* every key the evaluator can emit exists in **both** locales — a typo
  would otherwise surface as a raw `rev.headline_thrown` on screen;
* the English in the payload is exactly what the key renders to, so the
  fallback never contradicts the translation.
"""
import gzip
import json
import os
import re

import pytest

from capture.hslog_import import parsed_games
from eval import i18n
from eval.review import build_review

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EVAL_DIR = os.path.join(ROOT, "eval")
FIXTURES = os.path.join(ROOT, "tests", "logs", "fixtures")


@pytest.fixture(scope="module")
def locales():
    out = {}
    for lang in ("en", "uk"):
        with open(os.path.join(ROOT, "locales", f"{lang}.json"),
                  encoding="utf-8") as fh:
            out[lang] = json.load(fh)
    return out


def emitted_keys():
    """Every key literal handed to `msg()` / `render()` in `eval/`."""
    keys = set()
    for base, _dirs, names in os.walk(EVAL_DIR):
        if "__pycache__" in base:
            continue
        for name in sorted(names):
            if not name.endswith(".py"):
                continue
            with open(os.path.join(base, name), encoding="utf-8") as fh:
                src = fh.read()
            # Only whole literals: `msg("act." + kind)` builds its key
            # at runtime and is covered by the family test below.
            keys |= set(re.findall(
                r"(?:msg|render)\(\s*\"([\w.]+)\"\s*[,)]", src))
    return keys


# Built at runtime from the decision's kind, so no literal to scan for.
ACTION_KINDS = ("play", "attack", "hero_power", "location", "mulligan",
                "discover", "turn_start")


def walk_messages(node):
    """Every `{key, params, text}` dict anywhere in a review."""
    if isinstance(node, dict):
        if "key" in node and "text" in node and "params" in node:
            yield node
            return
        for value in node.values():
            yield from walk_messages(value)
    elif isinstance(node, list):
        for value in node:
            yield from walk_messages(value)


@pytest.fixture(scope="module")
def review():
    path = os.path.join(FIXTURES, "real_game1.log.gz")
    if not os.path.exists(path):
        pytest.skip("fixture missing")
    with gzip.open(path, "rt", encoding="utf-8", errors="replace") as fh:
        raw = fh.read()
    tmp = os.path.join(FIXTURES, "_i18n_tmp.log")
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(raw)
    try:
        _raw, events, summ, _p = list(parsed_games(tmp))[0]
    finally:
        os.remove(tmp)
    rev, _dec, _snap = build_review(events, game={"player_id": 1},
                                    deep=False)
    return rev


# ------------------------------------------------------------- the keys
def test_the_evaluator_emits_some_keys():
    keys = emitted_keys()
    assert len(keys) > 15, "i18n keys are not being emitted: %s" % keys


def test_every_emitted_key_exists_in_both_locales(locales):
    for lang, data in locales.items():
        missing = sorted(k for k in emitted_keys() if k not in data)
        assert not missing, "%s.json is missing %s" % (lang, missing)


def test_runtime_built_action_keys_exist(locales):
    """`msg("act." + kind)` has no literal for the scanner to find."""
    wanted = ["act." + k for k in ACTION_KINDS] + ["act.other"]
    for lang, data in locales.items():
        missing = [k for k in wanted if k not in data]
        assert not missing, "%s.json missing %s" % (lang, missing)


def test_render_falls_back_to_the_key_rather_than_raising():
    """A missing translation must not lose a review."""
    assert i18n.render("no.such.key.anywhere") == "no.such.key.anywhere"


def test_params_are_substituted_in_both_languages(locales):
    en = i18n.render("note.mana_unspent", {"turn": 7, "mana": 3}, "en")
    uk = i18n.render("note.mana_unspent", {"turn": 7, "mana": 3}, "uk")
    for text in (en, uk):
        assert "7" in text and "3" in text
        assert "{" not in text, "a hole was left unfilled: %r" % text
    assert en != uk, "uk.json is not translated for this key"


def test_msg_carries_key_params_and_english():
    m = i18n.msg("note.attacks_unused", n=2, turn=8)
    assert m["key"] == "note.attacks_unused"
    assert m["params"] == {"n": 2, "turn": 8}
    assert m["text"] == "2 attack(s) unused on turn 8"


# ---------------------------------------------------- a whole review
def test_a_real_review_carries_keyed_messages(review):
    found = list(walk_messages(review))
    assert len(found) > 3, "a review with no keyed message at all"


def test_every_message_in_a_review_resolves(review, locales):
    for m in walk_messages(review):
        for lang, data in locales.items():
            assert m["key"] in data, \
                "%s.json has no %s" % (lang, m["key"])


def test_the_english_fallback_is_what_the_key_renders(review):
    """If these ever disagree, the stored English is lying about what
    the translation says."""
    for m in walk_messages(review):
        assert m["text"] == i18n.render(m["key"], m["params"]), m["key"]


def test_the_report_mirrors_its_english(review):
    report = review["report"]
    assert report["headline"] == report["i18n"]["headline"]["text"]
    assert report["bullets"] == [m["text"] for m in report["i18n"]["bullets"]]
    assert report["caveats"] == [m["text"] for m in report["i18n"]["caveats"]]


def test_a_review_still_serialises(review):
    json.dumps(review)
