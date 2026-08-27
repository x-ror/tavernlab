import { useEffect, useMemo, useRef, useState } from 'react'
import { ComboBox, Item } from '@adobe/react-spectrum'
import * as api from '../api'

/* The card list is ~1150 names. It is fetched once per session and
 * filtered in the browser: a round trip per keystroke to a local
 * single-threaded HTTP server is worse than the memory. */
let cache = null
let inflight = null

function loadNames() {
  if (cache) return Promise.resolve(cache)
  if (!inflight) {
    inflight = api
      .cardNames(true)
      .then((d) => {
        cache = d.names || []
        return cache
      })
      .catch(() => [])
  }
  return inflight
}

export default function CardCombo({ label, onAdd, width = '100%' }) {
  const [names, setNames] = useState(cache || [])
  const [text, setText] = useState('')
  // Enter both commits the highlighted suggestion and ends the typed
  // custom value, and the two used to add the same card twice. The two
  // handlers fire in either order depending on where the key was
  // handled, so the guard is "did a selection just happen" rather than a
  // flag one of them resets.
  const lastSelect = useRef(0)

  useEffect(() => {
    let live = true
    loadNames().then((n) => live && setNames(n))
    return () => {
      live = false
    }
  }, [])

  const matches = useMemo(() => {
    const q = text.trim().toLowerCase()
    if (!q) return names.slice(0, 30).map((n) => ({ id: n, name: n }))
    return names
      .filter((n) => n.toLowerCase().includes(q))
      .slice(0, 30)
      .map((n) => ({ id: n, name: n }))
  }, [names, text])

  return (
    <ComboBox
      label={label}
      items={matches}
      inputValue={text}
      onInputChange={setText}
      allowsCustomValue
      width={width}
      onSelectionChange={(key) => {
        if (key) {
          lastSelect.current = Date.now()
          onAdd(String(key))
          setText('')
        }
      }}
      onKeyDown={(e) => {
        // Typing a name the picker did not offer still has to work: the
        // server resolves a name by prefix too. The check runs a tick
        // later so a suggestion the ComboBox commits on this same Enter
        // wins, instead of the card landing in the hand twice.
        if (e.key !== 'Enter') return
        const typed = text.trim()
        if (!typed) return
        setTimeout(() => {
          if (Date.now() - lastSelect.current < 100) return
          onAdd(typed)
          setText('')
        }, 0)
      }}
    >
      {(item) => <Item>{item.name}</Item>}
    </ComboBox>
  )
}
