"""Review text as (key, params), rendered to English on the way out.

The review is written once into `reviews.summary` and read back for
years, so it cannot be stored in whatever language the user happened to
have selected. It is stored as **both**: a stable key with its numbers,
which the UI translates, and the English rendering, which is the
fallback for rows written before a key existed.

The English is *derived* from `locales/en.json` rather than typed a
second time — otherwise the two drift and the fallback starts lying.

Nothing here is generated prose: every message is a template with named
holes, exactly as design §3.4 requires. `tests/test_review_i18n.py` pins
that every key emitted actually exists in both locales.
"""
import json
import os

_LOCALES = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "locales")

_CACHE = {}


def catalogue(lang="en"):
    """The flat `{key: template}` table for one language."""
    if lang not in _CACHE:
        path = os.path.join(_LOCALES, f"{lang}.json")
        try:
            with open(path, encoding="utf-8") as fh:
                _CACHE[lang] = json.load(fh)
        except (OSError, ValueError):
            _CACHE[lang] = {}
    return _CACHE[lang]


def _fill(value, lang):
    """A hole's value, which may itself be a message.

    "Turn {turn}: {detail}" holds another sentence in `detail`. Rendering
    that with `str()` would translate the frame and leave its filling in
    English — which is what the report used to do.
    """
    if isinstance(value, dict) and "key" in value:
        return render(value["key"], value.get("params"), lang)
    if isinstance(value, (list, tuple)):
        return "; ".join(_fill(v, lang) for v in value)
    return str(value)


def render(key, params=None, lang="en"):
    """Fill a template. Unknown key renders as the key, never as a
    traceback — a missing translation must not lose a review."""
    template = catalogue(lang).get(key)
    if template is None:
        return key
    for name, value in (params or {}).items():
        template = template.replace("{%s}" % name, _fill(value, lang))
    return template


def msg(key, **params):
    """One translatable message.

    `text` is what every existing consumer already reads; `key`/`params`
    are what the UI translates. Both come from one definition, so they
    cannot disagree.
    """
    return {"key": key, "params": params, "text": render(key, params)}


def text_of(message):
    """The English string of a message, or of a legacy plain string."""
    if isinstance(message, dict):
        return message.get("text") or message.get("key") or ""
    return str(message or "")


def texts_of(messages):
    return [text_of(m) for m in (messages or [])]
