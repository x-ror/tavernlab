"""Reconstruction and evaluation of a captured game.

Deliberately import-light: `eval.types` and `eval.visible` must stay
usable when `hs2` (the simulator) is not importable at all, because a
game full of unimplemented cards still has to reconstruct and still has
to get a review at `search_ok=0` (design §2.6).  Nothing heavy is
imported here — submodules are imported explicitly by their users.
"""
