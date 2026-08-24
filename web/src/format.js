/** Shared formatting. The games list, the coach home and the game header
 *  all describe the same row, so they must describe it the same way. */

export function fmtDate(v, lang) {
  if (!v) return '—'
  const d = new Date(typeof v === 'number' ? v * 1000 : v)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString(lang === 'uk' ? 'uk-UA' : 'en-US', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function reviewVariant(status) {
  if (status === 'ready') return 'positive'
  if (status === 'pending') return 'info'
  if (status === 'partial') return 'yellow'
  if (status === 'error') return 'negative'
  return 'neutral'
}

export function matchup(g, t) {
  const us = t(`class.${g.player_class || 'unknown'}`)
  const them = t(`class.${g.opponent_class || 'unknown'}`)
  return `${us} ${t('games.vs')} ${them}`
}

export function archetype(g, t) {
  if (!g.opponent_archetype) return null
  return t('games.arch_conf', {
    arch: g.opponent_archetype,
    conf: Math.round((g.opponent_archetype_conf || 0) * 100),
  })
}

export function pct(v, digits = 0) {
  if (v === null || v === undefined || Number.isNaN(v)) return '—'
  return `${(v * 100).toFixed(digits)}%`
}

export function signedPct(v) {
  if (v === null || v === undefined) return '—'
  const s = (v * 100).toFixed(1)
  return v > 0 ? `+${s}%` : `${s}%`
}

/** The format a deckstring declares, named for a human. */
export function formatName(fmt, t) {
  if (fmt === 'standard') return t('ui.deck.fmt_standard')
  if (fmt === 'wild') return t('ui.deck.fmt_wild')
  return t('ui.deck.fmt_unknown')
}

/** Why a deck will not resolve, in the player's terms.
 *
 *  `try_resolve` reports three different problems and they are not the
 *  same news: a card we cannot simulate, a card that is not legal in
 *  this format, and a card the corpus has never heard of. Collapsing
 *  them into one "Помилка" is what this replaces.
 */
export function deckProblem(info, t) {
  if (!info || info.pending || info.ok) return null
  if (info.illegal?.length) {
    return t('ui.deck.illegal', {
      fmt: formatName(info.format, t),
      cards: info.illegal.join(', '),
    })
  }
  if (info.unimplemented?.length) {
    return t('ui.deck.unimplemented', { cards: info.unimplemented.join(', ') })
  }
  if (info.missing?.length) {
    return t('ui.deck.missing', { cards: info.missing.join(', ') })
  }
  return info.error || t('ui.common.error')
}
