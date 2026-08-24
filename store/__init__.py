"""SQLite persistence for TavernLab (design §2.4/§2.5).

Import `open_store` rather than touching `sqlite3` anywhere else: every
write in the app has to go through the one writer thread this package
owns, or WAL's single-writer rule turns into "database is locked".
"""
from .db import DB_NAME, GEN_WHERE, SCHEMA_VERSION, Store, open_store

__all__ = ["Store", "open_store", "DB_NAME", "GEN_WHERE", "SCHEMA_VERSION"]
