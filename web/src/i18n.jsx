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
