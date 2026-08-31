/** Shared formatting: percentages, deltas, and the words for a format
 *  and for a deck the engine cannot field. Several screens say each of
 *  these, and they must say them the same way. */

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
  if (fmt === 'arena') return t('ui.deck.fmt_arena')
  return t('ui.deck.fmt_unknown')
}

/** Why a deck will not resolve, in the player's terms.
 *
 *  `/api/resolve` reports four different problems and they are not the
 *  same news: a card we cannot simulate, a card that is not legal in
 *  this format, a card the corpus has never heard of, and something that
 *  cannot go in a deck at all. Collapsing them into one "Помилка" is
 *  what this replaces.
 */
export function deckProblem(info, t) {
  if (!info || info.pending || info.ok) return null
  // A code that will not even decode comes back as a key, not a sentence:
  // the server serves both languages and cannot pick one.
  if (info.error_code) return t(`ui.deck.err_${info.error_code}`)
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
  if (info.not_deckable?.length) {
    return t('ui.deck.not_deckable', { cards: info.not_deckable.join(', ') })
  }
  return info.error || t('ui.common.error')
}

/** A count and its noun, in a language that inflects the noun.
 *
 *  Ukrainian has three forms and English two, and `{n} боїв` reads as a
 *  bug in the program rather than in the grammar: four games is "4 бої".
 *  `Intl.PluralRules` knows the categories; the strings supply the words.
 */
export function plural(t, key, n, lang) {
  const rules = new Intl.PluralRules(lang === 'uk' ? 'uk-UA' : 'en-GB')
  return t(`${key}_${rules.select(n)}`, { n })
}
