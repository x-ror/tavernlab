"""Solvers.

Each module exposes a narrow entry point that takes a `VisibleState` (plus
whatever context it needs) and returns a signal the classifier may or may
not be allowed to publish.  A solver never decides on its own whether its
answer is trustworthy — it reports the gates it satisfied and lets
`eval/classify.py` apply the publish rules from design §3.3.
"""
