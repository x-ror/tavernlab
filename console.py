"""Keep console output alive on a non-UTF-8 Windows console.

Windows hands Python the ANSI code page instead of UTF-8: on a Ukrainian
install that is cp1251, which covers every Cyrillic letter the interface
prints and none of ``→ └ ■ Δ ✓``.  A single one of those in a line raised
UnicodeEncodeError and took the process down — `watcher.py --help` died
before it printed anything.

The code page is left alone, because it renders the Cyrillic that is the
bulk of the output; only the unmappable glyphs degrade, and they degrade
to the ASCII they stand for rather than to ``?``.
"""
import codecs
import sys

ASCII_FALLBACK = {
    "→": "->", "←": "<-", "►": ">", "▸": ">", "■": "#", "☠": "!",
    "─": "-", "└": "`-", "├": "|-", "│": "|", "×": "x", "−": "-",
    "≥": ">=", "≤": "<=", "∧": "and", "Δ": "d", "✓": "+", "✗": "x",
    "⚠": "!", "…": "...", "«": '"', "»": '"', "’": "'", "—": "-",
}


def _replace(err):
    bad = err.object[err.start:err.end]
    return "".join(ASCII_FALLBACK.get(c, "?") for c in bad), err.end


codecs.register_error("tavernlab_ascii", _replace)


def init():
    """Install the fallback on stdout/stderr.

    Idempotent, and a no-op wherever the console already speaks UTF-8:
    every Linux/macOS terminal, and Windows under `PYTHONIOENCODING` or
    code page 65001.
    """
    for stream in (sys.stdout, sys.stderr):
        enc = (getattr(stream, "encoding", "") or "").replace("-", "").lower()
        if enc.startswith("utf") or not hasattr(stream, "reconfigure"):
            continue
        try:
            stream.reconfigure(errors="tavernlab_ascii")
        except (OSError, ValueError):   # a redirected/closed stream
            pass
