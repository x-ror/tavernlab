"""Strategic taggers (design §3.6).

A tagger takes a `VisibleState` (plus whatever context the log gives) and
returns `{tag, evidence, polarity}` — never an essay.  The bar the design
sets is blunt: **wrong strategic tags are worse than none**, so only two
ship here, both computable from what the log actually shows and both
pinned on fixture snapshots.

* `beatdown` — the race arithmetic behind "Fireball the face vs the
  3-health", stated as two clocks so the user can check it.
* `mulligan_keep` — the "6+ on the play" checklist that `watch_turn.py`
  already prototyped.

Everything else in §3.6's table (hand reading, play-around, sequencing,
discover greed) needs search or hidden-info sampling and is deliberately
absent rather than guessed at.
"""
import math

# A minion is a clock only if it can actually swing at a face.
_NO_FACE = ("FROZEN", "CANT_ATTACK", "DORMANT")

HEAVY_KEEP_COST = 6      # "6+ on the play" (design §3.6)

# A "clock" only means something once it is close enough to plan around,
# and only when it is clearly faster than the other side's. Without these
# two guards the tagger announces a beatdown off a lone 1/1 chipping for
# 40 turns — a tag nobody can check, which §3.6 says is worse than none.
MAX_MEANINGFUL_CLOCK = 6     # turns
MIN_CLOCK_GAP = 2            # turns


def _face_damage(vs, pid):
    """Damage this side can put on a face on its next turn."""
    total = 0
    for m in vs.board(pid):
        tags = m.tags or {}
        if tags.get("CARDTYPE") not in (None, "MINION"):
            continue
        if any(tags.get(t) for t in _NO_FACE):
            continue
        if (m.hp_left or 0) <= 0:
            continue
        swings = 2 if tags.get("WINDFURY") else 1
        total += (m.atk or 0) * swings
    hero = vs.hero(pid) or {}
    return total + (hero.get("atk") or 0)


def _clock(hp, dps):
    """Turns to kill, or None when this side has no clock at all."""
    if dps <= 0:
        return None
    return math.ceil(hp / dps)


def beatdown(vs, us_pid=None):
    """Who has to close the game.

    Two clocks, both from visible board damage against visible effective
    health.  Hidden cards are ignored on purpose: guessing at the
    opponent's hand is exactly the kind of weak evidence §3.6 says to
    omit.  `polarity` is +1 when we are the beatdown, -1 when they are,
    and 0 when the evidence does not separate them.
    """
    us = vs.us if us_pid is None else us_pid
    them = 2 if us == 1 else 1
    our_dps, their_dps = _face_damage(vs, us), _face_damage(vs, them)
    our_clock = _clock(vs.hp_total(them), our_dps)
    their_clock = _clock(vs.hp_total(us), their_dps)

    evidence = {
        "our_face_damage": our_dps, "their_face_damage": their_dps,
        "our_clock": our_clock, "their_clock": their_clock,
        "our_hp": vs.hp_total(us), "their_hp": vs.hp_total(them),
    }
    fastest = min(c for c in (our_clock, their_clock) if c is not None) \
        if (our_clock or their_clock) else None
    if fastest is None:
        return {"tag": "beatdown_unclear", "polarity": 0,
                "evidence": evidence,
                "text": "Neither side has a clock on the board."}
    if fastest > MAX_MEANINGFUL_CLOCK:
        return {"tag": "beatdown_unclear", "polarity": 0,
                "evidence": evidence,
                "text": (f"No real clock yet: the faster board still "
                         f"needs {fastest} turns.")}
    if our_clock is not None and their_clock is not None and \
            abs(our_clock - their_clock) < MIN_CLOCK_GAP:
        return {"tag": "beatdown_race", "polarity": 0,
                "evidence": evidence,
                "text": (f"Even race: {our_clock} turn(s) against "
                         f"{their_clock}.")}
    if their_clock is None or (our_clock is not None
                               and our_clock < their_clock):
        return {"tag": "beatdown_us", "polarity": 1, "evidence": evidence,
                "text": (f"You are the beatdown: you kill in "
                         f"{our_clock} turn(s) at {our_dps} face damage, "
                         f"they need "
                         f"{'no clock' if their_clock is None else their_clock}"
                         f". Trades are a concession.")}
    return {"tag": "beatdown_them", "polarity": -1,
            "evidence": evidence,
            "text": (f"They are the beatdown: they kill in "
                     f"{their_clock} turn(s) at {their_dps} face "
                     f"damage. Contest the board.")}


def removal_vs_face(vs, us_pid=None, target_is_face=None):
    """The §3.4 `removal_vs_face` template, stated with its arithmetic.

    Returns None rather than a hedge when the beatdown call is unclear —
    a strategic note nobody can check is worse than silence.
    """
    bd = beatdown(vs, us_pid)
    if bd["polarity"] == 0:
        return None
    ev = bd["evidence"]
    if bd["polarity"] > 0:
        why = (f"Face is the plan while you are the beatdown "
               f"(board attack {ev['our_face_damage']} vs "
               f"{ev['their_face_damage']}).")
    else:
        why = (f"Face is not the plan while they are the beatdown "
               f"(their clock is {ev['their_clock']} turn(s)).")
    if target_is_face is not None:
        agrees = (target_is_face == (bd["polarity"] > 0))
        why += " That matches this play." if agrees else \
            " This play went the other way."
    return {"tag": "removal_vs_face", "polarity": bd["polarity"],
            "evidence": ev, "text": why}


def mulligan_keep(choices, going_first=None, archetype=None,
                  card_name=None):
    """The `mull_keep_heavy` checklist: expensive keeps on the play.

    `choices` is the decision point's offered set,
    `[{"eid", "card_id", "picked", "cost"?, "name"?}]`.  Costs are only
    present for cards the reconstructor could name; unnamed entries are
    skipped rather than assumed cheap.
    """
    kept = [c for c in (choices or []) if c.get("picked")]
    heavy = [c for c in kept
             if isinstance(c.get("cost"), int)
             and c["cost"] >= HEAVY_KEEP_COST]
    evidence = {"kept": len(kept), "heavy": len(heavy),
                "going_first": going_first,
                "cards": [c.get("name") or c.get("card_id")
                          for c in heavy]}
    if not heavy:
        return None
    if going_first is False:
        # With the coin the curve is a turn cheaper; the checklist only
        # fires on the play.
        return None
    who = ", ".join(str(c) for c in evidence["cards"])
    vs_txt = f" vs {archetype}" if archetype else ""
    return {"tag": "mull_keep_heavy", "polarity": -1,
            "evidence": evidence,
            "text": (f"Kept {who} ({heavy[0]['cost']}+ mana) on the "
                     f"play{vs_txt}.")}


def all_tags(vs, us_pid=None):
    """Every tagger that has something defensible to say here."""
    out = []
    bd = beatdown(vs, us_pid)
    if bd["polarity"] != 0:
        out.append(bd)
    rvf = removal_vs_face(vs, us_pid)
    if rvf is not None:
        out.append(rvf)
    return out
