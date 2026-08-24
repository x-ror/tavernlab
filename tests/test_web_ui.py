"""The web UI, asserted against its own source.

Successor to `test_webui.py`, which guarded the vanilla `webui.html`
until that file was retired in favour of the React/Spectrum app in
`web/`. The rules are the same design rules, not style ones — they are
the ones that keep the product honest:

* nothing user-visible is hardcoded (§6.3.5) and both locales agree;
* no Mistake/Blunder/Best glyph is rendered as active — the greyed
  legend is the only place those words may appear (§3.3);
* the WP chart is hatched and carries its caveat (§3.3);
* `log.config` is a snippet the user pastes; we never write it (PR 12);
* nothing on the page talks to the network except this app.

No browser and no npm: every claim here is checkable by reading the
sources. The tests skip themselves if `web/src` is absent, so a checkout
without the front end still runs green.
"""
import json
import os
import re

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES = os.path.join(ROOT, "locales")
WEB_SRC = os.path.join(ROOT, "web", "src")

CYRILLIC = re.compile(r"[Ѐ-ӿ]")

# Skill glyphs. `eval/classify.py` gates every one of them behind
# `search_ok`, which is 0 in this build, so none may be rendered as a
# live label — they exist only as locale entries the legend greys out.
GATED_GLYPHS = ("Mistake", "Blunder", "Brilliant", "Inaccuracy",
                "Помилка", "Грубa помилка", "Блискуче")


def read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def strip_comments(src):
    """Prose for us, not for the user. These rules are about what gets
    rendered, so a glyph or a Ukrainian word inside a comment is not a
    violation — quoting the string you are replacing is normal."""
    src = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    return re.sub(r"^\s*//.*$", "", src, flags=re.M)


@pytest.fixture(scope="module")
def locales():
    return {lang: json.loads(read(os.path.join(LOCALES, lang + ".json")))
            for lang in ("en", "uk")}


@pytest.fixture(scope="module")
def web():
    """Every JS/JSX source file in the front end, keyed by repo path."""
    if not os.path.isdir(WEB_SRC):
        pytest.skip("web/src is not present in this checkout")
    out = {}
    for base, _dirs, names in os.walk(WEB_SRC):
        for name in sorted(names):
            if name.endswith((".js", ".jsx", ".css")):
                path = os.path.join(base, name)
                out[os.path.relpath(path, ROOT).replace("\\", "/")] = \
                    read(path)
    assert out, "web/src has no sources"
    return out


@pytest.fixture(scope="module")
def code(web):
    """Just the JS/JSX, without the stylesheet or the string table."""
    return {k: v for k, v in web.items()
            if k.endswith((".js", ".jsx")) and "strings.js" not in k}


@pytest.fixture(scope="module")
def ui_strings(web):
    """`web/src/strings.js` parsed into {lang: {key: value}}."""
    src = web["web/src/strings.js"]
    out = {}
    for lang in ("uk", "en"):
        block = re.search(r"\n  %s: \{(.*?)\n  \}," % lang, src, re.S)
        assert block, "strings.js has no %s block" % lang
        # No trailing `\n`: the block's last entry sits right against the
        # closing brace, so requiring one silently drops it.
        out[lang] = dict(re.findall(r"'([\w.]+)':\s*'(.*?)',",
                                    block.group(1)))
        assert out[lang], "%s block parsed empty" % lang
    return out


def used_keys(code):
    """Every key the app hands to `t()` as a literal."""
    keys = set()
    for src in code.values():
        keys |= set(re.findall(r"(?<![\w$.])t\(\s*'([\w.]+)'", src))
    return keys


# --------------------------------------------------------------- locales
def test_locales_parse_and_have_identical_key_sets(locales):
    en, uk = set(locales["en"]), set(locales["uk"])
    diff = en ^ uk
    assert not diff, (
        "locale key sets differ: only in en=%s only in uk=%s"
        % (sorted(en - uk), sorted(uk - en)))
    assert len(en) > 100, "suspiciously few keys: %d" % len(en)


def test_no_empty_translations(locales):
    for lang, data in locales.items():
        blank = [k for k, v in data.items() if not str(v).strip()]
        assert not blank, "%s has empty values: %s" % (lang, blank)


def test_placeholders_match_between_locales(locales):
    """`{n}` in one language must be `{n}` in the other, or the string
    silently loses its number."""
    for key, en in locales["en"].items():
        uk = locales["uk"][key]
        assert set(re.findall(r"\{(\w+)\}", en)) == \
            set(re.findall(r"\{(\w+)\}", uk)), key


def test_classify_labels_all_have_locale_entries(locales):
    from eval import classify
    for spec in classify.LABELS:
        for lang, data in locales.items():
            assert "label." + spec.key in data, \
                "%s.json has no label.%s" % (lang, spec.key)
    for lang, data in locales.items():
        assert "label.none" in data, lang


def test_locale_files_are_real_utf8(locales):
    uk_cyr = [k for k, v in locales["uk"].items() if CYRILLIC.search(v)]
    assert len(uk_cyr) > 60, "uk.json looks untranslated: %d" % len(uk_cyr)
    # The only Cyrillic in en.json is the Ukrainian endonym in the
    # language picker, which stays in its own language on purpose.
    en_cyr = [k for k, v in locales["en"].items() if CYRILLIC.search(v)]
    assert en_cyr == ["settings.lang_uk"], en_cyr


def test_existing_ukrainian_copy_is_preserved_verbatim(locales):
    """The four legacy tabs are the `uk` source; they were extracted, not
    retranslated."""
    uk = locales["uk"]
    assert uk["analyze.need_code"] == "Вставте деккод."
    assert uk["mull.keep"] == "ЛИШИТИ"
    assert uk["mull.toss"] == "СКИНУТИ"
    assert uk["opp.expect"] == "Очікуй:"
    assert uk["coach.weak"] == "Слабкі матчапи:"
    assert uk["tab.analyze"] == "Оцінка колоди"


# ------------------------------------------------- the app's own strings
def test_ui_strings_have_identical_key_sets(ui_strings):
    uk, en = set(ui_strings["uk"]), set(ui_strings["en"])
    diff = uk ^ en
    assert not diff, "strings.js differs: %s" % sorted(diff)
    assert len(uk) > 50, "suspiciously few keys: %d" % len(uk)


def test_ui_string_placeholders_match(ui_strings):
    for key, en in ui_strings["en"].items():
        uk = ui_strings["uk"][key]
        assert set(re.findall(r"\{(\w+)\}", en)) == \
            set(re.findall(r"\{(\w+)\}", uk)), key


def test_ui_strings_are_actually_translated(ui_strings):
    cyr = [k for k, v in ui_strings["uk"].items() if CYRILLIC.search(v)]
    assert len(cyr) > 40, "uk block looks untranslated: %d" % len(cyr)


# Russianisms that read as Ukrainian but are not, plus one calque. The
# list is short and unambiguous on purpose: a broad blocklist would fire
# on words that are legitimately Ukrainian in another sense.
RUSSIANISMS = {
    "лестниц": "рейтингова таблиця / сходи",
    "слідуюч": "наступний",
    "на протязі": "протягом",
    "по замовчуванню": "за замовчуванням",
    "приймати участь": "брати участь",
    "співпада": "збігається",
    "у якості": "як",
}


def _offending(text):
    low = text.lower()
    return [bad for bad in RUSSIANISMS if bad in low]


def test_the_shipped_locale_has_no_russianisms(locales):
    bad = []
    for key, value in locales["uk"].items():
        for word in _offending(value):
            bad.append(f"{key}: {word!r} -> {RUSSIANISMS[word]}")
    assert not bad, bad


def test_the_apps_own_strings_have_no_russianisms(ui_strings):
    bad = []
    for key, value in ui_strings["uk"].items():
        for word in _offending(value):
            bad.append(f"{key}: {word!r} -> {RUSSIANISMS[word]}")
    assert not bad, bad


def test_every_literal_key_exists(code, locales, ui_strings):
    """A key the app asks for and nobody defines renders as the key."""
    used = used_keys(code)
    assert used, "no t() calls found in web/src"
    known = set(locales["en"]) | set(ui_strings["en"])
    missing = sorted(k for k in used if k not in known)
    assert not missing, "undefined keys: %s" % missing


def test_runtime_key_families_are_translated(code, locales):
    """Keys built from data (`t(`class.${cls}`)`) still need entries."""
    classes = re.search(r"export const CLASSES = \{(.*?)\n\}",
                        code["web/src/classes.js"], re.S).group(1)
    keys = re.findall(r"^  ([A-Z]+):", classes, re.M)
    assert len(keys) == 11, keys
    wanted = ["class." + c for c in keys]
    wanted += ["class.unknown", "kind.unknown", "label.unknown"]
    wanted += ["result." + r for r in ("win", "loss", "tie", "unknown")]
    wanted += ["review_status." + s for s in
               ("pending", "ready", "partial", "error", "none")]
    for lang, data in locales.items():
        missing = [k for k in wanted if k not in data]
        assert not missing, "%s.json missing %s" % (lang, missing)


def test_no_user_visible_cyrillic_outside_the_string_table(code):
    """Ukrainian copy belongs in `strings.js` or `locales/`, never in a
    component — otherwise the English build silently speaks Ukrainian."""
    offenders = [path for path, src in code.items()
                 if CYRILLIC.search(strip_comments(src))]
    assert not offenders, offenders


# --------------------------------------------------------- honest labels
def test_no_gated_glyph_is_hardcoded(code):
    """Mistake/Blunder/Best may only be reached through `t('label.…')`,
    which the legend greys out. A literal in a component would ship the
    Chess.com glyph this build has not earned."""
    for path, src in code.items():
        body = strip_comments(src)
        for glyph in GATED_GLYPHS:
            assert glyph not in body, "%s hardcodes %r" % (path, glyph)


def test_the_legend_comes_from_the_server_gate_table(code):
    """`GET /api/labels`'s table (via the review payload) is the only
    source of what is publishable — the UI must not keep its own list."""
    legend = code["web/src/components/LabelLegend.jsx"]
    assert "legend.map" in legend, "the legend is not data-driven"
    assert "l.available" in legend, "the gate flag is ignored"
    assert "review.legend_needs" in legend, "the failed gate is not named"
    assert "labels_legend" in code["web/src/routes/GamePage.jsx"], \
        "the review payload's legend is never passed in"


def test_wp_chart_is_hatched_and_captioned(code):
    chart = code["web/src/components/WpChart.jsx"]
    assert "hatch" in chart, "the WP series is drawn solid"
    assert "strokeDasharray" in chart, "the WP line is not dashed"
    page = code["web/src/routes/GamePage.jsx"]
    assert "review.wp_hatched" in page, "the caveat is never rendered"


def test_no_chart_library(web):
    """The WP chart is hand-drawn SVG. A charting dependency would be the
    largest thing in the tree and would fight the hatching rule."""
    pkg = json.loads(read(os.path.join(ROOT, "web", "package.json")))
    deps = set(pkg.get("dependencies", {})) | set(pkg.get("devDependencies", {}))
    for banned in ("recharts", "d3", "chart.js", "victory", "plotly.js",
                   "echarts", "nivo"):
        assert not any(banned in d for d in deps), "%s is a dependency" % banned


def test_caveats_are_rendered_not_dropped(code):
    """`report.caveats` and a decision's own caveats are the product's
    honesty surface; both must reach the screen.

    The report's now arrive keyed, so the page reads them through
    `reportParts` — which is asserted here to actually look at
    `caveats`, or the chain could be silently cut in the middle."""
    page = code["web/src/routes/GamePage.jsx"]
    assert "reportParts(report, t)" in page, "the report is not unpacked"
    assert "<Caveats items={caveats}" in page, "report caveats are dropped"
    assert "expl.caveats" in page, "decision caveats are dropped"
    parts = code["web/src/msg.js"]
    assert "report.caveats" in parts, "reportParts ignores report.caveats"


# ------------------------------------------------------- legal behaviour
def test_logconfig_snippet_is_complete(code):
    dialog = code["web/src/components/ImportDialog.jsx"]
    for line in ("[Power]", "LogLevel=1", "FilePrinting=true",
                 "Verbose=true", "[Zone]"):
        assert line in dialog, "log.config snippet is missing %r" % line


def test_logconfig_is_never_written_for_the_user(code):
    """PR 12: show the snippet, never auto-write the file."""
    dialog = code["web/src/components/ImportDialog.jsx"]
    assert "import.manual_only" in dialog, "the disclaimer is not shown"
    for banned in ("writeFile", "showSaveFilePicker", "/api/logconfig"):
        assert banned not in dialog, "%s would write the file" % banned


def test_live_eval_is_off_by_default(code):
    """The setting is opt-in; the UI must read the stored value rather
    than defaulting the switch to on."""
    settings = code["web/src/routes/Settings.jsx"]
    assert "draft.live_eval === '1'" in settings, \
        "the live switch does not read the stored value"
    assert "settings.live_warn" in settings, "the warning is not shown"


def test_settings_fields_match_the_server_contract(code):
    """Every setting the user owns has a field; derived ones must not.

    `app.DERIVED_SETTINGS` is the server's own list of values it writes
    for itself, so the two cannot drift the way a copy in this file
    would."""
    import app
    settings = code["web/src/routes/Settings.jsx"]
    for key in app.DEFAULT_SETTINGS:
        if key in app.DERIVED_SETTINGS:
            assert key not in settings,                 "%r is derived; it must not be an editable field" % key
            continue
        assert key in settings, "Settings.jsx never offers %r" % key


# ------------------------------------------------------------ no network
def test_the_page_never_talks_to_anyone_but_this_app(web):
    """Everything is local. A CDN font, an analytics beacon or a lazy
    card-art fetch would each undo the "nothing is sent anywhere" line in
    the header — the art cache exists precisely so the browser asks this
    server, not Blizzard's."""
    allowed = re.compile(
        r"https?://(127\.0\.0\.1|localhost|www\.w3\.org/)")
    offenders = []
    for path, src in web.items():
        for m in re.finditer(r"https?://[^\s'\"`)]+", src):
            url = m.group(0)
            if not allowed.match(url):
                offenders.append("%s: %s" % (path, url))
    assert not offenders, offenders


def test_art_is_requested_from_this_server_only(code):
    classes = code["web/src/classes.js"]
    assert "/api/art/hero/" in classes
    assert "/api/art/tile/" in classes
    assert "hearthstonejson" not in classes, "art is hotlinked"


def test_missing_art_falls_back_to_a_drawn_crest(code):
    """The cache is optional. A broken-image box where a hero should be
    would make an un-fetched install look broken rather than plain."""
    portrait = code["web/src/components/HeroPortrait.jsx"]
    assert "onError" in portrait, "a failed portrait is never caught"
    assert "ClassCrest" in portrait, "there is no drawn fallback"
    tile = code["web/src/components/CardTile.jsx"]
    assert "onError" in tile, "a failed card tile is never caught"


def test_locales_are_fetched_from_the_server(code):
    i18n = code["web/src/i18n.jsx"]
    assert "/locales/" in i18n, "locales are never fetched"
    assert "navigator.language" in i18n, "no OS-language default"
