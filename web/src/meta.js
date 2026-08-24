import { useEffect, useState } from 'react'
import * as api from './api'

/* The gauntlet: the decks a rating is actually measured against.
 *
 * Fetched once per *format*, not once overall — Standard and Wild have
 * separate gauntlets, and asking the Standard one about a Wild deck's
 * opponents just returns names that never match, quietly dropping every
 * crest. One cache serves both the name→class lookup the matchup bars
 * need and the full deck lists the meta page draws.
 */
const cache = new Map() // format -> {decks: [...], byName: {name: class}}
const inflight = new Map()

function load(fmt) {
  if (cache.has(fmt)) return Promise.resolve(cache.get(fmt))
  if (!inflight.has(fmt)) {
    inflight.set(
      fmt,
      api
        .post('/api/meta', { format: fmt })
        .then((d) => {
          const decks = d.decks || []
          const entry = {
            decks,
            byName: Object.fromEntries(decks.map((x) => [x.name, x.cls])),
          }
          cache.set(fmt, entry)
          return entry
        })
        .catch(() => ({ decks: [], byName: {} })),
    )
  }
  return inflight.get(fmt)
}

function useMeta(fmt) {
  const key = fmt || 'standard'
  const [entry, setEntry] = useState(() => cache.get(key) || null)
  useEffect(() => {
    let live = true
    setEntry(cache.get(key) || null)
    load(key).then((e) => live && setEntry(e))
    return () => {
      live = false
    }
  }, [key])
  return entry
}

/** name -> class key for one format, `{}` until it arrives. */
export function useDeckClasses(fmt = 'standard') {
  return useMeta(fmt)?.byName || {}
}

/** The whole gauntlet for one format, `null` while it loads. */
export function useMetaDecks(fmt = 'standard') {
  return useMeta(fmt)?.decks || null
}

/** Cards per mana cost, 0…7+, for a curve. */
export function manaCurve(cardlist) {
  const bins = new Array(8).fill(0)
  for (const c of cardlist || []) {
    const cost = Math.max(0, Math.min(7, c.cost ?? 0))
    bins[cost] += c.n || 1
  }
  return bins
}
