import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import * as api from './api'

/* One shared app state: the settings and the deck under study.
 *
 * It exists because every screen here asks a question about the *same*
 * deck — rating, improvements, mulligan, opponent, coach notes — and in
 * the old UI the code had to be pasted into each of them separately.
 *
 * There is no telemetry gate any more. The simulator answers a mulligan
 * question in a fraction of a second from a cold start, so the screens
 * that need instrumented games just ask for them; the "analyse this deck
 * first" wall the Python build needed is gone with the wait it covered. */

const AppContext = createContext(null)

// The rating run is the one result worth keeping across a reload: it is
// the number the header and the gauntlet page both read, and re-running
// it on every refresh would be work the user did not ask for.
const RESULT_KEY = 'tavernlab.analysis'

function readJSON(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem(key) || '') ?? fallback
  } catch {
    return fallback
  }
}

function writeJSON(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* private mode: the app still works, it just forgets the last run */
  }
}

export function AppProvider({ children }) {
  const [settings, setSettings] = useState(null)
  const [deckCode, setDeckCodeState] = useState('')
  const [deckInfo, setDeckInfo] = useState(null) // /api/resolve answer
  const [analysis, setAnalysis] = useState(null) // /api/analyze result
  const resolveSeq = useRef(0)

  useEffect(() => {
    api.settingsApi
      .read()
      .then((d) => {
        setSettings(d.settings || {})
        if (d.settings?.deckstring) setDeckCodeState(d.settings.deckstring)
      })
      .catch(() => setSettings({}))
  }, [])

  const saveSettings = useCallback(async (patch) => {
    const d = await api.settingsApi.write(patch)
    setSettings(d.settings || {})
    return d.settings
  }, [])

  // Resolving is cheap and says immediately whether the engine can even
  // field this list, instead of failing several seconds into a run.
  useEffect(() => {
    const code = deckCode.trim()
    setAnalysis(null)
    if (!code) {
      setDeckInfo(null)
      return
    }
    const mine = ++resolveSeq.current
    setDeckInfo({ pending: true })
    const saved = readJSON(RESULT_KEY, null)
    if (saved && saved.code === code) setAnalysis(saved.result)

    api
      .post('/api/resolve', { code })
      .then((d) => resolveSeq.current === mine && setDeckInfo(d))
      .catch((e) => resolveSeq.current === mine && setDeckInfo({ ok: false, error: e.message }))
  }, [deckCode])

  const setDeckCode = useCallback(
    (code, { persist = true } = {}) => {
      setDeckCodeState(code)
      if (persist) saveSettings({ deckstring: code }).catch(() => {})
    },
    [saveSettings],
  )

  const markAnalysed = useCallback((code, result) => {
    setAnalysis(result)
    writeJSON(RESULT_KEY, { code: code.trim(), result })
  }, [])

  const value = useMemo(
    () => ({
      settings,
      saveSettings,
      deckCode,
      setDeckCode,
      deckInfo,
      // Recovered from the paste's `### Name` and kept in settings,
      // because the stored deckstring is normalised to the bare code.
      deckName: settings?.deck_name || '',
      analysis,
      markAnalysed,
    }),
    [settings, saveSettings, deckCode, setDeckCode, deckInfo, analysis, markAnalysed],
  )

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>
}

export function useApp() {
  const ctx = useContext(AppContext)
  if (!ctx) throw new Error('useApp outside AppProvider')
  return ctx
}

/** Hash routing without a router dependency: #/deck/mull */
export function useRoute() {
  const [hash, setHash] = useState(() => window.location.hash || '#/deck')
  useEffect(() => {
    const on = () => setHash(window.location.hash || '#/deck')
    window.addEventListener('hashchange', on)
    return () => window.removeEventListener('hashchange', on)
  }, [])
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean)
  return { parts, hash }
}

export function go(path) {
  window.location.hash = path.startsWith('#') ? path : `#/${path.replace(/^\//, '')}`
}
