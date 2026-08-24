"""In-game win probability: logistic model trained on simulator games.

Features are computed at the start of each of the current player's turns;
the label is whether that player ultimately won. Training data comes from
meta-vs-meta simulations; the fitted weights are stored in winprob.json.
"""
import json
import math
import os
import random

from .engine import Game
from .ai import Agent

_HERE = os.path.dirname(__file__)
WEIGHTS_PATH = os.path.join(_HERE, "winprob.json")

FEATS = ["bias", "hp_diff", "board_atk_diff", "board_hp_diff",
         "hand_diff", "deck_diff", "turn", "has_weapon",
         "taunt_hp_diff"]


def features(game, p):
    opp = p.opponent
    my_b = p.active_minions
    op_b = opp.active_minions
    return [
        1.0,
        ((p.hp + p.armor) - (opp.hp + opp.armor)) / 30.0,
        (sum(m.attack for m in my_b) - sum(m.attack for m in op_b)) / 10.0,
        (sum(m.health for m in my_b) - sum(m.health for m in op_b)) / 10.0,
        (len(p.hand) - len(opp.hand)) / 5.0,
        (len(p.deck) - len(opp.deck)) / 10.0,
        min(game.turn, 20) / 20.0,
        1.0 if p.weapon else 0.0,
        (sum(m.health for m in my_b if m.taunt)
         - sum(m.health for m in op_b if m.taunt)) / 8.0,
    ]


class SnapshotGame(Game):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.snaps = []          # (player_idx, feature_vector)

    def begin_turn(self, p):
        super().begin_turn(p)
        if not self.over and self.turn > 1:
            self.snaps.append((p.idx, features(self, p)))


def collect(decks_list, games_per_pair=60, seed=3):
    rows = []
    for a in decks_list:
        for b in decks_list:
            agents = [Agent(a.archetype), Agent(b.archetype)]
            for i in range(games_per_pair):
                g = SnapshotGame(a, b, seed=seed * 7_777_777 + i,
                                 agents=agents)
                w = g.run()
                if w is None:
                    continue
                for idx, f in g.snaps:
                    rows.append((f, 1.0 if idx == w else 0.0))
            seed += 1
    return rows


def train(rows, epochs=4, lr=0.3):
    w = [0.0] * len(FEATS)
    rnd = random.Random(1)
    for _ in range(epochs):
        rnd.shuffle(rows)
        for f, y in rows:
            z = sum(wi * xi for wi, xi in zip(w, f))
            pr = 1.0 / (1.0 + math.exp(-max(-30, min(30, z))))
            g = (pr - y) * lr
            for j in range(len(w)):
                w[j] -= g * f[j]
    return w


def evaluate(rows, w):
    ok = n = 0
    ll = 0.0
    for f, y in rows:
        z = sum(wi * xi for wi, xi in zip(w, f))
        pr = 1.0 / (1.0 + math.exp(-max(-30, min(30, z))))
        ok += 1 if (pr > 0.5) == (y > 0.5) else 0
        ll += -(y * math.log(max(pr, 1e-9)) +
                (1 - y) * math.log(max(1 - pr, 1e-9)))
        n += 1
    return ok / n, ll / n


def save(w):
    json.dump({"feats": FEATS, "w": w}, open(WEIGHTS_PATH, "w"))


_W = None


def winprob(game, p):
    """Probability that player p wins, from the trained model."""
    global _W
    if _W is None:
        _W = json.load(open(WEIGHTS_PATH))["w"]
    f = features(game, p)
    z = sum(wi * xi for wi, xi in zip(_W, f))
    return 1.0 / (1.0 + math.exp(-max(-30, min(30, z))))


def winprob_raw(feat_dict):
    """From a plain dict of raw values (for the app/live advisor).
    `my_turn` is YOUR turn number (1, 2, 3, …); internally the model
    was trained on engine half-turns, so it is converted here."""
    global _W
    if _W is None:
        _W = json.load(open(WEIGHTS_PATH))["w"]
    my_turn = feat_dict.get("my_turn", 5)
    f = [1.0,
         feat_dict.get("hp_diff", 0) / 30.0,
         feat_dict.get("board_atk_diff", 0) / 10.0,
         feat_dict.get("board_hp_diff", 0) / 10.0,
         feat_dict.get("hand_diff", 0) / 5.0,
         feat_dict.get("deck_diff", 0) / 10.0,
         min(my_turn * 2, 20) / 20.0,
         1.0 if feat_dict.get("has_weapon") else 0.0,
         feat_dict.get("taunt_hp_diff", 0) / 8.0]
    z = sum(wi * xi for wi, xi in zip(_W, f))
    return 1.0 / (1.0 + math.exp(-max(-30, min(30, z))))


def _live_minions(views):
    out = []
    for e in views:
        tags = getattr(e, "tags", None) or {}
        if tags.get("CARDTYPE") not in (None, "MINION"):
            continue
        if tags.get("DORMANT"):
            continue
        out.append(e)
    return out


def features_from_visible(vs, us_pid=None):
    """`features()` computed off a VisibleState instead of a live `Game`.

    The turn term is the whole point of this helper existing.  The HS
    `GameEntity` TURN tag and `Game.turn` both increment **once per player
    turn**, so the term is `min(vs.turn, 20) / 20` — the same as
    `features()`.  `winprob_raw` takes *your* turn number and doubles it;
    feeding it `vs.turn` would double-count and shift the model by up to
    ten half-turns.
    """
    us = vs.us if us_pid is None else us_pid
    them = 2 if us == 1 else 1

    def side(pid):
        b = _live_minions(vs.boards.get(pid, []))
        hero = vs.heroes.get(pid, {})
        return {
            "hp": (hero.get("hp") or 0) + (hero.get("armor") or 0),
            "atk": sum(e.atk or 0 for e in b),
            "hp_board": sum((e.hp_left or 0) for e in b),
            "taunt_hp": sum((e.hp_left or 0) for e in b
                            if (e.tags or {}).get("TAUNT")),
            "hand": len(vs.hands.get(pid, [])),
            "deck": vs.deck_counts.get(pid, 0),
            "weapon": bool(vs.weapons.get(pid)),
        }

    a, b = side(us), side(them)
    return [
        1.0,
        (a["hp"] - b["hp"]) / 30.0,
        (a["atk"] - b["atk"]) / 10.0,
        (a["hp_board"] - b["hp_board"]) / 10.0,
        (a["hand"] - b["hand"]) / 5.0,
        (a["deck"] - b["deck"]) / 10.0,
        min(vs.turn, 20) / 20.0,
        1.0 if a["weapon"] else 0.0,
        (a["taunt_hp"] - b["taunt_hp"]) / 8.0,
    ]


def wp_from_visible(vs, us_pid=None):
    """p(win) for `us_pid` from a VisibleState.

    Display only.  These are 8 board features plus a bias, trained on
    scripted self-play: they do not see card identity, combo, quests or
    hidden cards.  Every chart drawn from this must be hatched, and no
    play may be ranked or labelled by its delta (design §3.3).
    """
    global _W
    if _W is None:
        _W = json.load(open(WEIGHTS_PATH))["w"]
    f = features_from_visible(vs, us_pid)
    z = sum(wi * xi for wi, xi in zip(_W, f))
    return 1.0 / (1.0 + math.exp(-max(-30, min(30, z))))
