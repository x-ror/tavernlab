import { createContext, useContext, useEffect, useMemo, useState } from 'react'
import { EXTRA } from './strings'

/* The server owns `locales/{lang}.json` (nested); this UI adds its own
 * flat keys on top. Both are read through one `t()` so a string can move
 * from one to the other without touching a component. */

function flatten(obj, prefix = '', out = {}) {
  for (const [k, v] of Object.entries(obj || {})) {
    const key = prefix ? `${prefix}.${k}` : k
    if (v && typeof v === 'object' && !Array.isArray(v)) flatten(v, key, out)
    else out[key] = v
  }
  return out
}

const SUPPORTED = ['uk', 'en']

export function pickLang(setting) {
  const want = (setting || navigator.language || 'en').slice(0, 2).toLowerCase()
  return SUPPORTED.includes(want) ? want : 'en'
}

const I18nContext = createContext({ lang: 'en', t: (k) => k })

export function I18nProvider({ lang, children }) {
  const [server, setServer] = useState({})

  useEffect(() => {
    let live = true
    fetch(`/locales/${lang}.json`)
      .then((r) => (r.ok ? r.json() : {}))
      .then((d) => live && setServer(flatten(d)))
      .catch(() => live && setServer({}))
    return () => {
      live = false
    }
  }, [lang])

  const value = useMemo(() => {
    const dict = { ...server, ...EXTRA[lang] }
    const t = (key, vars) => {
      let s = dict[key]
      if (s === undefined) return key
      if (vars) {
        for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(String(v))
      }
      return s
    }
    return { lang, t, ready: Object.keys(server).length > 0 }
  }, [lang, server])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useT() {
  return useContext(I18nContext)
}

/* A line the server built: a key, and the values that fill it in.
 *
 * `{k, p}` rather than a sentence, because the server is one process
 * serving two languages and the advice it writes is read on this page.
 * A value that is itself `{k}` is another key — a class name, a verdict —
 * and one that is `{k, p}` is a whole nested phrase, which is how a plan
 * line carries "your Chillwind Yeti" as one translatable unit.
 *
 * See `cli/src/watch/advice.rs`, which is the other half of this. */
export function renderLine(t, line) {
  if (!line) return ''
  if (typeof line === 'string') return line
  const vars = {}
  for (const [name, value] of Object.entries(line.p || {})) {
    vars[name] = value && typeof value === 'object' ? renderLine(t, value) : String(value)
  }
  return t(line.k, vars)
}
