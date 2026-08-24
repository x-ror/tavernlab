/* Messages from the evaluator arrive keyed.
 *
 * `eval/i18n.py` emits `{key, params, text}`: the key and its numbers so
 * this UI can translate them, and the English rendering as the fallback
 * for reviews stored before a key existed. Anything older is a plain
 * string and passes straight through.
 */
/** A hole's value may itself be a message — "Turn {turn}: {detail}"
 *  holds another sentence. Translating only the frame is how the report
 *  ended up half Ukrainian. */
function fill(value, t) {
  if (Array.isArray(value)) return value.map((v) => fill(v, t)).join('; ')
  if (value && typeof value === 'object' && value.key) return msgText(value, t)
  return value
}

export function msgText(m, t) {
  if (m === null || m === undefined) return ''
  if (typeof m === 'string') return m
  if (typeof m !== 'object') return String(m)
  if (m.key) {
    const params = Object.fromEntries(
      Object.entries(m.params || {}).map(([k, v]) => [k, fill(v, t)]),
    )
    const translated = t(m.key, params)
    // `t()` echoes the key when nothing defines it; the stored English
    // is a better answer than showing the player `rev.headline_loss`.
    if (translated !== m.key) return translated
  }
  return m.text || m.key || ''
}

export function msgList(list, t) {
  return (list || []).map((m) => msgText(m, t)).filter(Boolean)
}

/** The report's translatable half, falling back to its English one. */
export function reportParts(report, t) {
  if (!report) return { headline: '', bullets: [], caveats: [] }
  const i18n = report.i18n || {}
  return {
    headline: msgText(i18n.headline ?? report.headline, t),
    bullets: msgList(i18n.bullets ?? report.bullets, t),
    caveats: msgList(i18n.caveats ?? report.caveats, t),
  }
}
