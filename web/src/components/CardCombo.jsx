import { useEffect, useMemo, useState } from 'react'
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
          onAdd(String(key))
          setText('')
        }
      }}
      onKeyDown={(e) => {
        // Custom values matter here: the engine knows cards the picker
        // list may not, and the server resolves the name anyway.
        if (e.key === 'Enter' && text.trim()) {
          onAdd(text.trim())
          setText('')
        }
      }}
    >
      {(item) => <Item>{item.name}</Item>}
    </ComboBox>
  )
}
