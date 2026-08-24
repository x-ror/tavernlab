"""Where TavernLab keeps the user's data.

Design §2.9 says everything lives on the user's own disk and nothing is
sent anywhere; this module decides *where* on that disk.  The previous
answer was "next to the program", which meant a git checkout slowly
filling with a SQLite database, a rotating log and a cache directory,
and a frozen build whose games disappeared the day you replaced the exe.

Resolution order:

  1. an explicit ``--workdir`` from the caller (this module never sees it),
  2. ``TAVERNLAB_HOME``, for portable installs and for tests,
  3. the per-user data directory the OS already has.

Nothing here creates anything until :func:`ensure_home` is called.
"""
import os
import sys

APP_NAME = "TavernLab"


def default_home():
    """The data directory, without touching the filesystem."""
    env = os.environ.get("TAVERNLAB_HOME")
    if env:
        return os.path.abspath(os.path.expanduser(env))
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    elif sys.platform == "darwin":
        base = os.path.join(os.path.expanduser("~"), "Library",
                            "Application Support")
    else:
        base = (os.environ.get("XDG_DATA_HOME")
                or os.path.join(os.path.expanduser("~"), ".local", "share"))
    return os.path.join(base, APP_NAME)


def ensure_home(path=None):
    """`default_home()`, created if missing. Returns the path."""
    home = os.path.abspath(path or default_home())
    os.makedirs(home, exist_ok=True)
    return home


def in_home(*parts):
    """A path inside the data directory, which is created if missing."""
    return os.path.join(ensure_home(), *parts)
