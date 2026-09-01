import { useEffect, useRef, useState } from 'react'
import { Flex, Text } from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { renderLine, useT } from '../i18n'
import { ErrorNote, Panel } from '../components/ui'
import CardTile from '../components/CardTile'
import { heroArt } from '../classes'

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

/* `memreader`'s entity shape (`cardId`/`atk`/`health`/`cost`, `name`
 * resolved server-side) onto `CardTile`'s (`card_id`/`atk`/`health`/
 * `damage`/`cost`/`tags`) -- the same art-behind-a-scrim tile the deck lab
 * and the replay use, so a card looks like the same card everywhere in the
 * app. `damage` is always 0: memreader doesn't read `TAG_DAMAGE` yet, so
 * `health` here is the printed/buffed maximum, not remaining health after
 * a trade -- a known gap, not a silent wrong number (the tile just can't
 * show a minion as hurt yet). No keyword tags either, for the same reason
 * (Taunt/Divine Shield/... aren't read from `List<Tag>` yet), so a tile
 * never claims a mark it can't back up. */
function toTile(e) {
  if (!e) return null
  return {
    card_id: e.cardId,
    name: e.name,
    atk: e.atk,
    health: e.health,
    damage: 0,
    cost: e.cost,
    tags: {},
  }
}

function Crystals({ mana, manaMax }) {
  const max = Math.max(0, manaMax || 0)
  if (!max) return null
  const cur = Math.max(0, Math.min(max, mana ?? 0))
  return (
    <span className="tl-crystals" aria-hidden="true">
      {Array.from({ length: max }, (_, i) => (
        <i key={i} className={`tl-crystal${i < cur ? '' : ' is-spent'}`} />
      ))}
    </span>
  )
}

function BoardRow({ label, entities, kind }) {
  const { t } = useT()
  return (
    <div>
      <div className="tl-row-label">
        {label} · {entities.length}
        {kind === 'minion' ? '/7' : ''}
      </div>
      <div className="tl-board-row">
        {entities.length ? (
          entities.map((e, i) => <CardTile key={e.addr || i} entity={toTile(e)} kind={kind} />)
        ) : (
          <span className="tl-empty-hint">{t('ui.live.empty')}</span>
        )}
      </div>
    </div>
  )
}

/* The hero as a medallion, not a flat minion tile -- the one piece of the
 * real board this owes more to `HeroPortrait`/`.tl-portrait` than to
 * `CardTile`. Keyed on `hero.class` (server-resolved in
 * cli/src/serve/memory.rs from the hero's card id), not the card id
 * itself: every hero *skin* is its own id (`HERO_11`, `HERO_11bp`, ...)
 * the art cache was never built against, while `/api/art/hero/{class}`
 * has real art for all eleven classes. A hero with no class here (an
 * unresolved corpus lookup) just shows the ring on an empty disc. */
function HeroMedallion({ hero, armor, current }) {
  const [failed, setFailed] = useState(false)
  const art = hero?.class && !failed ? heroArt(hero.class) : null
  return (
    <div className={`tl-hero-wrap${current ? ' is-current' : ''}`} title={hero?.name}>
      <div className="tl-hero-portrait">
        {art && <img src={art} alt="" onError={() => setFailed(true)} />}
      </div>
      <span className="tl-hero-ring" aria-hidden="true" />
      {hero && (
        <>
          <span className="tl-cs tl-atk">{hero.atk ?? 0}</span>
          <span className="tl-cs tl-hp">{hero.health ?? '?'}</span>
        </>
      )}
      {armor > 0 && <span className="tl-armor">{armor}</span>}
    </div>
  )
}

function SideHead({ side }) {
  const { t } = useT()
  return (
    <div className="tl-side-head">
      <HeroMedallion hero={side.hero} armor={side.armor} current={side.current} />
      <div className="tl-gear">
        {side.heroPower && <CardTile entity={toTile(side.heroPower)} kind="hand" />}
        {side.weapon && <CardTile entity={toTile(side.weapon)} kind="minion" />}
      </div>
      <div className="tl-side-stats">
        <span className="tl-chip">
          {t('ui.live.mana')} {side.mana ?? '—'}/{side.manaMax ?? '—'}
        </span>
        <Crystals mana={side.mana} manaMax={side.manaMax} />
        <span className="tl-chip">
          {t('ui.live.deck')} {side.deck ?? 0}
        </span>
        <span className="tl-chip">
          {t('ui.live.graveyard')} {side.graveyard ?? 0}
        </span>
      </div>
    </div>
  )
}

/* Mirrored on purpose: the opponent's hand sits above their board, yours
 * below yours, the two `play` rows meeting at the divider -- the same
 * reading order the game's own table has, not an arbitrary stack of
 * sections. `flip` reverses the row order for the opponent's half. */
function SideBoard({ side, mine, flip }) {
  const { t } = useT()
  const name = (
    <div className="tl-side-name">
      {mine ? t('ui.live.you') : t('ui.live.them')}
      {side.name ? ` — ${side.name}` : ''}
      {side.current ? ` · ${t('ui.live.turn_marker')}` : ''}
    </div>
  )
  const secrets = side.secret?.length > 0 && (
    <Flex gap="size-100" wrap>
      {side.secret.map((s, i) => (
        <span key={s.addr || i} className="tl-secret-pip">
          🔒 {s.name || s.cardId}
        </span>
      ))}
    </Flex>
  )
  const hand = <BoardRow label={t('ui.live.hand')} entities={side.hand || []} kind="hand" />
  const play = <BoardRow label={t('ui.live.play')} entities={side.play || []} kind="minion" />
  const head = <SideHead side={side} />
  const rows = flip ? [head, hand, secrets, play] : [play, secrets, hand, head]
  return (
    <div className="tl-board-side">
      {name}
      {rows.map((r, i) => (
        <div key={i}>{r}</div>
      ))}
    </div>
  )
}

function MemoryBoard({ snap, battletag }) {
  const { t } = useT()
  const sides = snap?.sides || []
  if (!sides.length) return null
  const me = battletag.split('#')[0].toLowerCase()
  const meSide = (me && sides.find((s) => (s.name || '').toLowerCase() === me)) || sides[1] || sides[0]
  const oppSide = sides.find((s) => s !== meSide)
  return (
    <div style={{ marginTop: 8 }}>
      <div className="tl-row-label">{t('ui.live.board')}</div>
      <div className="tl-felt">
        {oppSide && (
          <>
            <SideBoard side={oppSide} mine={false} flip />
            <hr className="tl-rule" />
          </>
        )}
        <SideBoard side={meSide} mine />
      </div>
      <details style={{ marginTop: 10 }}>
        <summary style={{ cursor: 'pointer', color: 'var(--tl-muted)', fontSize: '.8rem' }}>
          {t('ui.live.raw_json')}
        </summary>
        {/* Every tag `memreader` read, not just the curated fields above --
         * for pointing at exactly what a card's `rawTags` say without a
         * terminal session, which is the whole reason `rawTags` exists
         * (see memreader/README.md). A screenshot of this is itself useful
         * evidence when chasing an offset. */}
        <pre className="tl-mono" style={{ marginTop: 6, maxHeight: 320, overflow: 'auto' }}>
          {JSON.stringify(snap, null, 2)}
        </pre>
      </details>
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
