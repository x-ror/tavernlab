"""Read-only Hearthstone log capture.

MVP is **import-first**: `hslog_import` turns a Power.log slice into
canonical events.  The live tailer is v1 (design A15 / PR 6).
"""
from .events import EVENT_TYPES, canonicalize, walk        # noqa: F401
