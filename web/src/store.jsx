import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import * as api from './api'

/* One shared app state: settings, the deck under study, and the games
 * list. It exists because four screens (rating, improvements, mulligan,
 * opponent) all operate on the *same* deck — in the old UI the code had
 * to be pasted into each of them separately. */

const AppContext = createContext(null)

// The server keeps telemetry in advisor_cache/<hash>.json and exposes no
// "is it there" route. Two things stand in for one:
//
//  * a probe — `/api/mull` with an empty hand answers with the matchup if
//    the cache is warm and with the "analyse first" message if it is not.
//    Without it, telemetry produced in another browser (or before a
//    cache clear) read as missing and the tab was gated for no reason;
//  * localStorage — the rates and coach notes themselves, which live
//    only in the job result and would otherwise be lost on reload.
const ANALYSED_KEY = 'tavernlab.analysed'
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
    /* private mode: the probe still answers, only the cache is lost */
  }
}

function readAnalysed() {
  return readJSON(ANALYSED_KEY, {}) || {}
}

export function AppProvider({ children }) {
  const [settings, setSettings] = useState(null)
  const [deckCode, setDeckCodeState] = useState('')
  const [deckInfo, setDeckInfo] = useState(null) // /api/resolve answer
  const [analysis, setAnalysis] = useState(null) // job_analyze result
  const [analysed, setAnalysed] = useState(readAnalysed)
  const [probed, setProbed] = useState(null) // server says the cache is warm
  const [gamesList, setGamesList] = useState(null)
  const [gamesError, setGamesError] = useState(null)
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

  const refreshGames = useCallback(async (filters = {}) => {
    try {
      const d = await api.games.list({ limit: 100, ...filters })
      setGamesList(d.games || [])
      setGamesError(null)
      return d.games || []
    } catch (e) {
      setGamesError(e.message)
      return []
    }
  }, [])

  useEffect(() => {
    refreshGames()
  }, [refreshGames])

  const saveSettings = useCallback(async (patch) => {
    const d = await api.settingsApi.write(patch)
    setSettings(d.settings || {})
    return d.settings
  }, [])

  // Resolving is cheap and tells the user *immediately* whether the
  // engine can even simulate this list, instead of failing 15 s into a
  // gauntlet run.
  useEffect(() => {
    const code = deckCode.trim()
    setAnalysis(null)
    if (!code) {
      setDeckInfo(null)
      return
    }
    const mine = ++resolveSeq.current
    setDeckInfo({ pending: true })
    setProbed(null)
    // A previous run's rates, if this browser has them for this deck.
    const saved = readJSON(RESULT_KEY, null)
    if (saved && saved.code === code) setAnalysis(saved.result)

    api
      .post('/api/resolve', { code })
      .then((d) => resolveSeq.current === mine && setDeckInfo(d))
      .catch((e) => resolveSeq.current === mine && setDeckInfo({ ok: false, error: e.message }))

    // An empty hand asks the question without asking about a card.
    api
      .post('/api/mull', { code, opp: 'DRUID', hand: [] })
      .then(() => resolveSeq.current === mine && setProbed(true))
      .catch(() => resolveSeq.current === mine && setProbed(false))
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
    setProbed(true)
    writeJSON(RESULT_KEY, { code: code.trim(), result })
    setAnalysed((prev) => {
      const next = { ...prev, [code.trim()]: Date.now() }
      writeJSON(ANALYSED_KEY, next)
      return next
    })
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
      // The probe is authoritative when it has answered; the local marks
      // only cover the gap before it does.
      hasTelemetry:
        probed === null ? Boolean(analysis) || Boolean(analysed[deckCode.trim()]) : probed,
      gamesList,
      gamesError,
      refreshGames,
    }),
    [
      settings,
      saveSettings,
      deckCode,
      setDeckCode,
      deckInfo,
      analysis,
      markAnalysed,
      analysed,
      probed,
      gamesList,
      gamesError,
      refreshGames,
    ],
  )

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>
}

export function useApp() {
  const ctx = useContext(AppContext)
  if (!ctx) throw new Error('useApp outside AppProvider')
  return ctx
}

/** Hash routing without a router dependency: #/games/12/replay */
export function useRoute() {
  const [hash, setHash] = useState(() => window.location.hash || '#/coach')
  useEffect(() => {
    const on = () => setHash(window.location.hash || '#/coach')
    window.addEventListener('hashchange', on)
    return () => window.removeEventListener('hashchange', on)
  }, [])
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean)
  return { parts, hash }
}

export function go(path) {
  window.location.hash = path.startsWith('#') ? path : `#/${path.replace(/^\//, '')}`
}
