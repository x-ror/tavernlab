import { useEffect, useRef, useState } from 'react'
import { Flex, Text } from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { renderLine, useT } from '../i18n'
import { ErrorNote, Panel } from '../components/ui'

/* Advice while you play: mulligan, the opponent, this turn, and the
 * board the memory reader can see. One panel. The log watcher and
 * /api/memory stay separate on the server (the tracker never reads
 * process memory); this page asks both and draws them together.
 */

const POLL_MS = 1000
/* A snapshot is a heap scan — seconds, not milliseconds. Start the next
 * one a few seconds after the last finishes, never overlap. */
const MEMORY_GAP_MS = 4000

export default function Live() {
  const { t } = useT()
  const { settings } = useApp()
  const [live, setLive] = useState(null)
  const [mem, setMem] = useState(null)
  const [error, setError] = useState(null)
  const starting = useRef(false)

  useEffect(() => {
    let on = true
    const tick = () =>
      api
        .get('/api/live')
        .then(async (d) => {
          if (!on) return
          if (!d.running && !starting.current) {
            starting.current = true
            try {
              d = await api.post('/api/live', { action: 'start' })
            } catch (e) {
              if (on) setError(e.message)
            }
          }
          if (on) setLive(d)
        })
        .catch(() => {})
    tick()
    const id = setInterval(tick, POLL_MS)
    return () => {
      on = false
      clearInterval(id)
    }
  }, [])

  useEffect(() => {
    let on = true
    let wait = 0
    const tick = async () => {
      try {
        const d = await api.get('/api/memory')
        if (on) setMem(d)
      } catch {
        if (on) setMem(null)
      }
      if (on) wait = setTimeout(tick, MEMORY_GAP_MS)
    }
    tick()
    return () => {
      on = false
      clearTimeout(wait)
    }
  }, [])

  const deck = settings?.deckstring || ''
  const title = live?.title?.length
    ? live.title.map((part) => renderLine(t, part)).join(' — ')
    : t('ui.live.title')

  return (
    <Panel title={title}>
      {error && <ErrorNote error={error} />}
      {!deck && (
        <Text UNSAFE_style={{ display: 'block', marginBottom: 12, color: 'var(--tl-warn)' }}>
          {t('ui.live.no_deck')}
        </Text>
      )}
      {live?.note && (
        <Text
          UNSAFE_style={{
            display: 'block',
            marginBottom: 12,
            color: live.note.k === 'live.note.log_capped' ? 'var(--tl-warn)' : 'var(--tl-muted)',
          }}
        >
          {renderLine(t, live.note)}
        </Text>
      )}
      <Advice live={live} hasBoard={!!mem?.sides?.length} />
      <MemoryBoard snap={mem} battletag={settings?.battletag || ''} />
    </Panel>
  )
}

/* One section per heading, in the order the watcher built them: what to
 * do first, then what the opponent looks like, then the position it read.
 * A heading with no lines under it is never sent, so nothing here renders
 * an empty block that reads as "no advice".
 *
 * The turn is numbered and the rest is not, and that is the whole of the
 * difference in weight this screen needs: a plan is a sequence, where the
 * third line only makes sense after the second, while the position and the
 * opponent read are a set of facts in no particular order. */
function Advice({ live, hasBoard }) {
  const { t } = useT()
  if (!live?.sections?.length) {
    if (hasBoard) return null
    return <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>{t('ui.live.nothing_yet')}</Text>
  }
  return (
    <Flex direction="column">
      {live.sections.map((s) => {
        const ordered = s.heading === 'live.head.turn'
        const List = ordered ? 'ol' : 'ul'
        return (
          <div key={s.heading} style={{ marginBottom: 18 }}>
            <div
              style={{
                fontSize: '.75rem',
                letterSpacing: '.08em',
                color: 'var(--tl-muted)',
                marginBottom: 6,
              }}
            >
              {t(s.heading)}
            </div>
            <List
              style={{
                margin: 0,
                paddingLeft: 22,
                lineHeight: 1.7,
                fontSize: ordered ? '1.05rem' : '1rem',
              }}
            >
              {s.lines.map((line, i) => (
                <Line key={i} line={line} ordered={ordered} />
              ))}
            </List>
          </div>
        )
      })}
    </Flex>
  )
}

/* A caveat is not a step.
 *
 * The plan carries lines that say what it could not see — that the mana was
 * guessed, that there is no deck to draw from — and rendering them as the
 * next thing to do would be advice to do something that is not a play. They
 * keep their place in the list, because that is where they were built, and
 * lose the number and the weight. */
const CAVEATS = new Set(['live.plan.mana_guessed', 'live.plan.no_deck'])

function memCard(e) {
  if (!e) return '—'
  const name = e.cardId || `#${e.id}`
  if (e.atk != null || e.health != null) return `${name} ${e.atk ?? '?'}/${e.health ?? '?'}`
  if (e.cost != null) return `${name} (${e.cost})`
  return name
}

function MemoryBoard({ snap, battletag }) {
  const { t } = useT()
  const sides = snap?.sides || []
  if (!sides.length) return null
  const me = battletag.split('#')[0].toLowerCase()
  const ordered = [...sides].sort((a, b) => {
    const am = me && (a.name || '').toLowerCase() === me ? 0 : 1
    const bm = me && (b.name || '').toLowerCase() === me ? 0 : 1
    return am - bm || a.playerId - b.playerId
  })
  return (
    <div style={{ marginTop: 8 }}>
      <div
        style={{
          fontSize: '.75rem',
          letterSpacing: '.08em',
          color: 'var(--tl-muted)',
          marginBottom: 10,
        }}
      >
        {t('ui.live.board')}
      </div>
      <Flex direction="column" gap="size-150">
        {ordered.map((s) => {
          const mine = me && (s.name || '').toLowerCase() === me
          const row = (label, body) =>
            body ? (
              <div style={{ fontSize: '.95rem', lineHeight: 1.55 }}>
                <span style={{ color: 'var(--tl-muted)', marginRight: 8 }}>{label}</span>
                {body}
              </div>
            ) : null
          const list = (arr) =>
            arr?.length ? arr.map(memCard).join(', ') : t('ui.live.empty')
          return (
            <div key={s.playerId}>
              <Text UNSAFE_style={{ fontWeight: 600 }}>
                {mine ? t('ui.live.you') : t('ui.live.them')}
                {s.name ? ` — ${s.name}` : ''}
                {s.manaMax
                  ? `  ${t('ui.live.mana')} ${s.mana ?? '—'}/${s.manaMax}`
                  : ''}
              </Text>
              {row(t('ui.live.hero'), memCard(s.hero))}
              {s.heroPower ? row(t('ui.live.hero_power'), memCard(s.heroPower)) : null}
              {s.weapon ? row(t('ui.live.weapon'), memCard(s.weapon)) : null}
              {row(t('ui.live.play'), list(s.play))}
              {row(t('ui.live.hand'), list(s.hand))}
              {s.secret?.length ? row(t('ui.live.secret'), list(s.secret)) : null}
              {row(t('ui.live.deck'), String(s.deck ?? 0))}
            </div>
          )
        })}
      </Flex>
    </div>
  )
}

function Line({ line, ordered }) {
  const { t } = useT()
  const caveat = ordered && CAVEATS.has(line?.k)
  return (
    <li
      style={
        caveat
          ? { listStyle: 'none', marginLeft: -22, color: 'var(--tl-muted)', fontSize: '.85rem' }
          : undefined
      }
    >
      {renderLine(t, line)}
    </li>
  )
}
