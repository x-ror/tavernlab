#!/usr/bin/env python3
"""ToS guard (design PR 12): fail if input-injection ever lands.

The product's legal posture is HSDT-class — read-only logs, an
information-only overlay, no memory reads, no packet interception, no
input injection.  That posture is only as good as the code, so this runs
in CI and on demand:

    python3 scripts/tools_tos_grep.py  # exit 1 on any hit

Adding one of these APIs is not a lint failure to be silenced; it is a
product-ending risk (design "Non-goals").
"""
import os
import re
import sys

BANNED = {
    "SendInput": "Windows input injection",
    "keybd_event": "Windows input injection",
    "mouse_event": "Windows input injection",
    "pynput": "input injection / global hooks",
    "pyautogui": "input injection",
    "frida": "process instrumentation",
    "ReadProcessMemory": "memory reading",
    "OpenProcess": "memory reading",
    "ctypes.windll.user32": "input injection surface",
    "scapy": "packet interception",
    "pydivert": "packet interception",
    "raw_socket": "packet interception",
}

SKIP_DIRS = {".git", "__pycache__", "build", "dist", "node_modules",
             ".venv", "venv"}
SKIP_FILES = {os.path.basename(__file__)}
EXTS = (".py", ".js", ".html", ".sh", ".spec", ".toml", ".cfg")


REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def scan(root=None):
    root = root or REPO      # the repository, whatever the cwd is
    hits = []
    pattern = re.compile("|".join(re.escape(k) for k in BANNED))
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            if name in SKIP_FILES or not name.endswith(EXTS):
                continue
            path = os.path.join(dirpath, name)
            try:
                with open(path, encoding="utf-8", errors="replace") as fh:
                    for i, line in enumerate(fh, 1):
                        m = pattern.search(line)
                        if m:
                            hits.append((path, i, m.group(0),
                                         BANNED[m.group(0)],
                                         line.strip()[:90]))
            except OSError:
                continue
    return hits


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    hits = scan(root)
    for path, line, token, why, text in hits:
        print(f"{path}:{line}: {token} ({why})\n    {text}")
    if hits:
        print(f"\n{len(hits)} banned API reference(s). "
              f"TavernLab is read-only-logs by design.")
        return 1
    print(f"ToS grep clean: none of {len(BANNED)} banned APIs present.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
