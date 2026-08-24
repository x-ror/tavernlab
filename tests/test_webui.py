"""PR 11/11b/12 — the web UI, asserted against the files themselves.

No browser: every claim here is checkable by reading `webui.html` and the
two locale files. The rules under test are design rules, not style ones:

* nothing user-visible is hardcoded (§6.3.5) and both locales agree;
* the new GET routes go through `getJSON`, never the POST-only `api()`
  (§2.7);
* no Mistake/Blunder/Best glyph is rendered as active — the greyed
  legend is the only place those words may appear (§3.3);
* the WP chart is hatched and carries its caveat (§3.3);
* `log.config` is a snippet the user pastes; we never write it (PR 12).
"""
import json
import os
import re

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WEBUI = os.path.join(ROOT, "webui.html")
LOCALES = os.path.join(ROOT, "locales")

NEW_PANES = ("games", "review", "replay", "import", "settings")
ALL_PANES = NEW_PANES + ("analyze", "mull", "opp", "coach")

# `api()` is POST-only. Every path handed to it must be a POST route.
POST_PATHS = {
    "/api/analyze", "/api/optimize", "/api/mull", "/api/predict",
    "/api/cardnames", "/api/winprob", "/api/resolve", "/api/meta",
    "/api/settings", "/api/import/log", "/api/import/last_session",
    "/api/cards",
}

# Existing contract: ids and helpers other code and users depend on.
KEEP_IDS = ("code", "bAnalyze", "bOptimize", "bMull", "bPredict",
            "cardlist", "outA", "outO", "outM", "outP", "outC", "progA")

CYRILLIC = re.compile(r"[Ѐ-ӿ]")


def read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


@pytest.fixture(scope="module")
def html():
    return read(WEBUI)


@pytest.fixture(scope="module")
def locales():
    out = {}
    for lang in ("en", "uk"):
        out[lang] = json.loads(read(os.path.join(LOCALES, lang + ".json")))
    return out


def used_keys(html):
    """Every key the page asks `t()` / `data-i18n` for, literally."""
    attrs = re.findall(r'data-i18n(?:-[a-z]+)?="([^"]+)"', html)
    calls = re.findall(r'\bt\(\s*"([^"]+)"\s*[,)]', html)
    return set(attrs) | set(calls)


def pane_block(html, pane):
    """The static markup of one pane, `<div class="pane" ...>` to its
    explicit end comment."""
    marker = 'id="p-%s"' % pane
    at = html.index(marker)
    start = html.rindex("<div", 0, at)
    end = html.index("<!-- /p-%s -->" % pane)
    return html[start:end]


# --------------------------------------------------------------- i18n
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


def test_every_used_key_exists_in_both_locales(html, locales):
    used = used_keys(html)
    assert used, "no i18n keys found in webui.html"
    for lang, data in locales.items():
        missing = sorted(used - set(data))
        assert not missing, "%s.json is missing %s" % (lang, missing)


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


def test_prefixed_families_are_translated(html, locales):
    """Keys built at runtime by `tk(prefix, value)` still need entries."""
    classes = re.search(r"const CLASSES=\[(.*?)\];", html, re.S).group(1)
    classes = re.findall(r'"([A-Z]+)"', classes)
    assert len(classes) == 11, classes
    wanted = ["class." + c for c in classes]
    wanted += ["class.unknown", "kind.unknown", "label.unknown"]
    wanted += ["result." + r for r in ("win", "loss", "tie", "unknown")]
    wanted += ["review_status." + s for s in
               ("pending", "ready", "partial", "error", "none")]
    for lang, data in locales.items():
        missing = [k for k in wanted if k not in data]
        assert not missing, "%s.json missing %s" % (lang, missing)


def test_locale_files_are_real_utf8(locales):
    uk_cyr = [k for k, v in locales["uk"].items() if CYRILLIC.search(v)]
    assert len(uk_cyr) > 60, "uk.json looks untranslated: %d" % len(uk_cyr)
    # the only Cyrillic in en.json is the Ukrainian endonym in the
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
    assert "12 топ-колод" in uk["analyze.caption"]


def test_language_bootstrap(html):
    assert "navigator.language" in html, "no OS-language default"
    assert "/locales/" in html, "locales are never fetched"
    assert "localStorage" in html, "language is not mirrored to storage"
    for guard in re.findall(r"localStorage\.[a-zA-Z]+\([^)]*\)", html):
        # every access has to sit inside a try/catch
        at = html.index(guard)
        window = html[max(0, at - 120):at]
        assert "try{" in window, "unguarded localStorage: %s" % guard


# --------------------------------------------------- no hardcoded copy
def test_new_panes_carry_no_hardcoded_text(html):
    """Static markup in the new panes is empty; `data-i18n` fills it."""
    for pane in ALL_PANES:
        block = pane_block(html, pane)
        block = re.sub(r'<pre[^>]*id="logcfg".*?</pre>', "", block,
                       flags=re.S)
        block = re.sub(r'<code[^>]*id="cfgPath".*?</code>', "", block,
                       flags=re.S)
        text = " ".join(re.sub(r"<[^>]*>", " ", block).split())
        assert not text, "pane p-%s has hardcoded text: %r" % (pane, text)


def test_new_panes_are_translated(html):
    """Static markup goes through `data-i18n`; the panes drawn entirely
    from JS (review) go through `t()` in their renderer."""
    for pane in NEW_PANES:
        block = pane_block(html, pane)
        if re.sub(r"<[^>]*>", "", block).strip() or "data-i18n" in block:
            assert "data-i18n" in block, "pane p-%s has no data-i18n" % pane
    for fn in ("renderReview", "renderGames", "legendHtml", "decHtml",
               "turnHtml", "momentsHtml", "sideHtml", "logHtml",
               "inspectHtml", "altsHtml"):
        body = re.search(r"function %s\(.*?\n}" % fn, html, re.S)
        assert body, "missing renderer %s" % fn
        assert re.search(r"\bt[k]?\(", body.group(0)), \
            "%s renders untranslated text" % fn


def test_no_cyrillic_outside_style_and_bootstrap(html):
    body = re.sub(r"<style>.*?</style>", "", html, flags=re.S)
    body = re.sub(r"async function setLang.*?\n}", "", body, flags=re.S)
    found = CYRILLIC.findall(body)
    assert not found, "hardcoded Ukrainian left in webui.html: %s" % found


# ------------------------------------------------------------- routing
def test_getjson_exists(html):
    assert re.search(r"\basync function getJSON\(", html), \
        "PR 11 requires a getJSON() helper; api() is POST-only"
    assert re.search(r"async function api\(path,body\)\{const r=await "
                     r'fetch\(path,\{method:"POST"', html), \
        "the POST api() helper changed shape"


def test_api_helper_is_only_used_for_post_routes(html):
    for path in re.findall(r'\bapi\(\s*"([^"]+)"', html):
        assert path in POST_PATHS, \
            "%s is a GET route but goes through the POST api()" % path


def test_new_get_routes_go_through_getjson(html):
    assert re.search(r'getJSON\("/api/games\?', html), "games list"
    assert 'getJSON("/api/labels")' in html, "labels legend"
    assert 'getJSON("/api/settings")' in html, "settings"
    assert re.search(r'getJSON\(gameUrl\([^)]*"review"\)', html), "review"
    assert re.search(r'function gameUrl\(', html)
    # the review job is a POST, and it is polled with the existing poll()
    assert re.search(r'api\(path,\{\}\)', html), "analyze POST"
    assert re.search(r'poll\(d\.job,\$\("progR"\)\)', html), "job poll"


def test_job_poll_still_uses_get_directly(html):
    assert 'fetch("/api/job/"+jid)' in html


# ------------------------------------------------------- log.config S0
def test_logconfig_snippet_is_complete(html):
    snippet = re.search(r'<pre[^>]*id="logcfg"[^>]*>(.*?)</pre>', html,
                        re.S).group(1)
    for line in ("[Power]", "LogLevel=1", "FilePrinting=true",
                 "ConsolePrinting=false", "ScreenPrinting=false",
                 "Verbose=true", "[Zone]"):
        assert line in snippet, "log.config snippet is missing %r" % line
    assert "%LOCALAPPDATA%\\Blizzard\\Hearthstone\\log.config" in html
    assert 'id="bCopyCfg"' in html, "no copy button"


def test_logconfig_is_never_written_for_the_user(html):
    """PR 12: show the snippet, never auto-write the file."""
    assert not re.search(r"fetch\([^)]*log\.?config", html, re.I), \
        "the page posts log.config somewhere"
    for forbidden in ("/api/logconfig", "/api/log_config", "writeFile",
                      "write_log_config", "auto-write", "autowrite"):
        assert forbidden not in html, "found %r" % forbidden
    # the copy button only reads the snippet out of the DOM
    assert '$("logcfg").textContent' in html


# ---------------------------------------------------- honesty (§3.1/3.3)
def test_legend_is_rendered_from_the_server_gate_table(html):
    assert "labels_legend" in html, "legend must come from the review JSON"
    assert '"/api/labels"' in html, "…or from /api/labels"
    assert "function legendHtml(" in html
    assert "review.legend_soon" in html, \
        "greyed labels need the 'coming when calibrated' note"
    assert "review.legend_needs" in html, "gates are not shown"
    assert re.search(r'class="lg \$\{on\?"on":"off"\}"', html), \
        "the legend does not grey out unavailable labels"


def test_no_glyph_is_hardcoded_as_active(html):
    """Chess.com glyphs may only exist as locale entries reached through
    the greyed legend — never as markup in the page."""
    hits = re.findall(r"\b(blunder|brilliant|best|inaccuracy)\b", html,
                      re.I)
    assert not hits, "glyph words hardcoded in webui.html: %s" % hits


def test_missed_lethal_is_the_only_strong_treatment(html):
    assert 'd.label==="missed_lethal"' in html
    assert "review.missed_lethal_head" in html
    assert "review.approx_note" in html, "approx lethal must say so"
    assert "dec.lethal" in html, "missed lethal needs its own style"


def test_search_off_is_stated(html):
    assert "review.search_off" in html
    assert "label_reason" in html, "the hidden-label reason must be shown"
    assert "review.no_label" in html


def test_wp_chart_is_hatched_and_captioned(html):
    assert "wp_series" in html
    assert re.search(r"<pattern id=\"wpHatch\"", html), \
        "the WP area needs an SVG hatch <pattern>"
    assert 'url(#wpHatch)' in html, "the hatch is never used as a fill"
    assert '"logistic_v1"' in html, "hatching is not tied to the source"
    assert "wp_caveat" in html, "wp_caveat() must be printed by the chart"
    assert "review.wp_hatched" in html
    assert "<svg" in html and "viewBox" in html, "hand-rolled SVG only"


def test_no_chart_library_or_cdn(html):
    assert "http://" not in html.replace("http://www.w3.org", "")
    assert "https://" not in html
    assert "<script src" not in html


# ------------------------------------------------------ settings (S10)
def test_live_eval_is_off_by_default_and_hint_mode_is_v1(html):
    block = pane_block(html, "settings")
    live = re.search(r'<input type="checkbox" id="sLive"[^>]*>', block)
    assert live, "no live-eval checkbox"
    assert "checked" not in live.group(0), \
        "live evaluation must default to off (design §6.4)"
    hint = re.search(r'<input type="radio"[^>]*id="sModeHint"[^>]*>', block,
                     re.S)
    assert hint and "disabled" in hint.group(0), \
        "hint-only lethal is v1: the radio has to be disabled"
    assert "settings.v1_tag" in block
    line = re.search(r'<input type="radio"[^>]*id="sModeLine"[^>]*>', block,
                     re.S)
    assert line and "checked" in line.group(0), \
        "the default live mode is the full lethal line"
    assert "settings.live_warn" in block, "no opt-in warning copy"


def test_settings_fields_match_the_server_contract(html):
    for field in ("logs_dir", "player_name", "deckstring", "language",
                  "live_eval", "live_lethal_mode"):
        assert field in html, "settings field %s is not posted" % field
    assert "settings.player_name_hint" in html, "battletag is optional"


# -------------------------------------------------- existing behaviour
def test_existing_ids_survive(html):
    for eid in KEEP_IDS:
        assert 'id="%s"' % eid in html, "lost element #%s" % eid
    for pane in ALL_PANES:
        assert 'id="p-%s"' % pane in html
    assert "function bars(rates)" in html
    assert "async function poll(jid,progEl)" in html


def test_api_data_is_escaped_before_innerhtml(html):
    assert "function esc(s)" in html
    for raw in ("${d.error}", "${e.message}", "${c.card}", "${x.deck}",
                "${rep.headline}", "${d.label_reason}", "${c.text}",
                "${e.name}", "${g.review_blocked}", "${m.detail}",
                "${cardTip(e)}", "${entName(e)}"):
        assert raw not in html, "unescaped API value in markup: %s" % raw


def test_cardnames_asks_for_unimplemented_names(html):
    assert re.search(r'api\("/api/cardnames",\{all:true\}\)', html), \
        "replay/review name lookups need all:true (PR 12)"

# ----------------------------------------------------- S3 replay (§6.2)
def test_replay_reads_snapshots_through_getjson(html):
    assert re.search(r'getJSON\(gameUrl\([^)]*"replay"\)', html), \
        "the replay must come from GET /api/games/{id}/replay"
    assert "event_seq" in html, "snapshots are keyed by event_seq"
    assert "d.snapshots" in html


def test_replay_has_a_full_scrubber(html):
    block = pane_block(html, "replay")
    for eid in ("rpFirst", "rpPrevTurn", "rpPrev", "rpPlay", "rpNext",
                "rpNextTurn", "rpLast", "rpScrub", "rpLog"):
        assert 'id="%s"' % eid in block, "replay control %s missing" % eid
    assert 'type="range"' in block, "no scrubber"
    assert "const PLAY_MS=500" in html, "playback must be 2 actions/s"
    assert re.search(r"setInterval\(.*?PLAY_MS\)", html, re.S), \
        "play/pause is not wired to PLAY_MS"
    assert "function rpTurn(dir)" in html, "no prev/next turn"


def test_replay_board_is_seven_plus_seven_with_heroes_and_hands(html):
    for fn in ("function boardHtml(", "function sideHtml(",
               "function heroHtml(", "function backsHtml(",
               "function entHtml(", "function manaText("):
        assert fn in html, "missing %s" % fn
    for field in ("boards", "hands", "heroes", "weapons", "mana",
                  "deck_counts", "secrets", "corpses"):
        assert field in html, "VisibleState.%s is never rendered" % field
    # ours face-up, theirs as backs with a count
    assert "replay.hand_hidden" in html
    assert re.search(r"mine\?\s*\n?\s*\(hand\.length", html), \
        "our hand must render face-up, theirs as backs"


def test_replay_handles_string_player_ids(html):
    """`{1: ...}` comes back from JSON as `{"1": ...}`."""
    assert "map[String(pid)]" in html, \
        "player ids arrive as strings through JSON"


def test_replay_badges_state_what_is_off(html):
    meta = re.search(r"function replayMetaHtml\(.*?\n}", html, re.S)
    assert meta, "no replay meta renderer"
    body = meta.group(0)
    assert "replay.search_off" in body, "search off badge is required"
    assert re.search(r"s\.lethal_ok\?\s*\"\":", body), \
        "the lethal-off badge must be conditional on lethal_ok"
    assert "wpCaveat()" in body, "a WP number needs its caveat"


def test_entity_hover_uses_the_cards_route_batched(html):
    assert "const CARD_BATCH=400" in html, "the server caps ids at 400"
    assert re.search(r'api\("/api/cards",\{ids:ids\.slice', html), \
        "card lookups must be batched"
    assert "function cardTip(" in html
    assert re.search(r'title="\$\{esc\(cardTip\(', html), \
        "entity tooltips are not wired"
    assert "implemented===false" in html, \
        "unimplemented cards must still be named and marked"


# -------------------------------------------------- S4 inspector (§6.2)
def test_inspector_shows_the_lethal_line_and_never_ranks(html):
    assert "function altsHtml(" in html
    assert "inspector.lethal_line" in html, "no lethal alternative row"
    assert "inspector.no_dwp" in html, "the ΔWP disclaimer is required"
    assert "lethalPlanFor(" in html, "the lethal line comes from a moment"
    assert "delta_wp" not in html, \
        "delta_wp must not reach the UI in this build"
    assert "sort(" not in html.split("function altsHtml(")[1][:600], \
        "alternatives must not be ranked"


def test_inspector_always_shows_why_we_might_be_wrong(html):
    fn = re.search(r"function whyWrongHtml\(d\)\{.*?\n}", html, re.S)
    assert fn, "no why-we-might-be-wrong block"
    body = fn.group(0)
    assert "review.search_off" in body, "search_ok=0 must always be stated"
    assert "d.label_reason" in body
    assert re.search(r'\n  lines\.push\(t\("review\.search_off"\)\);',
                     body), \
        "search_ok=0 must be pushed unconditionally, not inside an if"


def test_explanation_strategic_and_mulligan_choices_are_rendered(html):
    assert "inspector.strategic" in html, "beatdown/mull reads are dropped"
    assert "e.strategic" in html
    assert "function choicesHtml(" in html
    assert "c.picked" in html, "kept vs tossed comes from choices[].picked"
    assert "inspector.mull_kept" in html and "inspector.mull_tossed" in html


def test_review_and_replay_are_wired_both_ways(html):
    assert 'data-seq="${esc(d.seq)}"' in html, \
        "a ply in the review must link into the replay"
    assert re.search(r'openReplay\(reviewId,Number\(', html), \
        "the ply link does not snap the replay to that seq"
    assert "function idxForSeq(" in html
    assert "replay.to_review" in html, "no way back from replay to review"


# --------------------------------------------------------- Q6 gating
def test_unreviewable_games_are_tagged_not_clickable(html):
    assert "g.reviewable===false" in html, "Q6 gating is not implemented"
    assert "g.review_blocked" in html, "the server reason is not shown"
    assert "games.blocked" in html
    rows = re.search(r'\$\("outG"\)\.querySelectorAll\("tbody tr"\)'
                     r'\.forEach\(tr=>\{.*?\n  \}\);', html, re.S)
    assert rows, "row wiring not found"
    body = rows.group(0)
    assert 'data-blocked' in body and body.index("data-blocked") < \
        body.index("tr.onclick"), \
        "blocked rows must be skipped before the click handler"


def test_blocked_games_hide_the_analyse_button(html):
    assert "function blockedHtml(" in html
    assert "review.blocked" in html
    fn = re.search(r"function renderReview\(\)\{.*?\n}", html, re.S)
    assert fn, "renderReview not found"
    body = fn.group(0)
    assert "const blocked=" in body
    assert "if(!blocked)" in body, \
        "the Analyse button must be suppressed for a blocked game"
    assert "reviewHeader(r.status===\"ready\"&&!blocked)" in body, \
        "re-analyse must be suppressed for a blocked game too"

