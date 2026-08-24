import { Flex, Text } from '@adobe/react-spectrum'
import { useT } from '../i18n'
import { classWash } from '../classes'
import CardTile from './CardTile'
import HeroPortrait from './HeroPortrait'

/* Renders a `VisibleState` — what the log actually revealed, not a
 * simulated game. Hidden cards stay hidden: the opponent's hand is a
 * count of face-down cards, and an unnamed minion says so instead of
 * guessing at a name. */

const num = (v) => (typeof v === 'number' ? v : 0)
const keyOf = (pid) => String(pid)

export default function Board({ visible, side, cls }) {
  const { t } = useT()
  if (!visible) return null
  const us = visible.us || 1
  const them = us === 1 ? 2 : 1
  const pid = side === 'us' ? us : them

  const hero = visible.heroes?.[keyOf(pid)] || {}
  const mana = visible.mana?.[keyOf(pid)] || {}
  const weapon = visible.weapons?.[keyOf(pid)]
  const board = visible.boards?.[keyOf(pid)] || []
  const hand = visible.hands?.[keyOf(pid)] || []
  const secrets = visible.secrets?.[keyOf(pid)]
  const deck = visible.deck_counts?.[keyOf(pid)]
  const corpses = visible.corpses?.[keyOf(pid)]

  const left = Math.max(
    0,
    num(mana.crystals) - num(mana.used) - num(mana.overload) + num(mana.temp),
  )
  const crystals = num(mana.crystals)
  const active = visible.current_player === pid

  return (
    <div
      className={`tl-side${active ? ' is-active' : ''}`}
      style={{ background: `linear-gradient(180deg, ${classWash(cls, 0.07)}, transparent 120px), var(--tl-surface)` }}
    >
      <Flex direction="row" alignItems="center" gap="size-200" wrap>
        <HeroPortrait cls={cls} size={46} flip={side === 'them'} title={cls} />
        <Flex direction="column" gap="size-25">
          <Text UNSAFE_style={{ fontWeight: 700, fontSize: '.9rem' }}>
            {side === 'us' ? t('replay.side_us') : t('replay.side_them')}
          </Text>
          <Flex direction="row" gap="size-100" alignItems="center" wrap>
            <span className="tl-hero-plate">
              <HeartPip />
              {num(hero.hp)}
              {num(hero.armor) > 0 && <ArmorPip n={hero.armor} />}
              {num(hero.atk) > 0 && <AtkPip n={hero.atk} />}
            </span>
            <ManaPips left={left} of={crystals} />
          </Flex>
        </Flex>

        <span style={{ flex: '1 1 auto' }} />

        <Flex direction="row" gap="size-150" alignItems="center" wrap>
          {weapon && (
            <Meta label={t('replay.weapon', { a: num(weapon.atk), d: num(weapon.durability ?? weapon.hp) })} />
          )}
          {deck !== undefined && <Meta label={t('replay.deck', { n: deck })} />}
          {secrets ? (
            <Meta label={t('replay.secrets', { n: Array.isArray(secrets) ? secrets.length : secrets })} />
          ) : null}
          {corpses ? <Meta label={t('replay.corpses', { n: corpses })} /> : null}
        </Flex>
      </Flex>

      <Flex direction="row" gap="size-100" wrap marginTop="size-200">
        {board.length === 0 ? (
          <div className="tl-empty-board">{t('replay.empty_board')}</div>
        ) : (
          board.map((e) => <CardTile key={e.eid} entity={e} kind="minion" />)
        )}
      </Flex>

      {(side === 'us' ? hand.length > 0 : true) && (
        <Flex direction="row" gap="size-75" wrap marginTop="size-200" alignItems="center">
          {side === 'us' ? (
            hand.map((e) => <CardTile key={e.eid} entity={e} kind="hand" />)
          ) : (
            <Text UNSAFE_style={{ fontSize: '.78rem', color: 'var(--tl-faint)' }}>
              {t('replay.hand_hidden', { n: hand.length })}
            </Text>
          )}
        </Flex>
      )}
    </div>
  )
}

function Meta({ label }) {
  return (
    <Text UNSAFE_style={{ fontSize: '.76rem', color: 'var(--tl-muted)' }}>{label}</Text>
  )
}

/* Mana as pips rather than "3/7". Ten crystals is the game's own unit of
   measurement, and an unspent one is the leak this app keeps pointing at
   — worth being able to count at a glance. */
function ManaPips({ left, of }) {
  const { t } = useT()
  if (!of) return null
  const total = Math.min(10, of)
  return (
    <span
      className="tl-hero-plate"
      style={{ gap: 3, paddingInline: 8 }}
      title={t('replay.mana', { n: left, of })}
    >
      {Array.from({ length: total }, (_, i) => (
        <i
          key={i}
          style={{
            width: 8,
            height: 8,
            borderRadius: '50%',
            display: 'block',
            border: '1px solid rgba(0,0,0,.6)',
            background:
              i < left
                ? 'radial-gradient(circle at 35% 30%, #7ec8f5, #1560a8 75%)'
                : 'rgba(255,255,255,.12)',
          }}
        />
      ))}
      {of > 10 && <span style={{ fontSize: '.72rem' }}>+{of - 10}</span>}
    </span>
  )
}

function HeartPip() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" aria-hidden="true">
      <path
        d="M12 21C7 17.5 3 14.2 3 9.9 3 7.1 5.2 5 7.9 5c1.6 0 3.1.8 4.1 2 1-1.2 2.5-2 4.1-2C18.8 5 21 7.1 21 9.9c0 4.3-4 7.6-9 11.1Z"
        fill="#c0272d"
        stroke="rgba(0,0,0,.6)"
        strokeWidth="1"
      />
    </svg>
  )
}

function ArmorPip({ n }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 2 }}>
      <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 2 20 5v7c0 5-3.4 8.4-8 10-4.6-1.6-8-5-8-10V5Z" fill="#9aa4b0" stroke="rgba(0,0,0,.6)" />
      </svg>
      {n}
    </span>
  )
}

function AtkPip({ n }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 2 }}>
      <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 2 14 7v8h-4V7Zm-5 13h10l-1 2H8Zm4 3h2v4h-2Z" fill="#d8a53a" stroke="rgba(0,0,0,.6)" strokeWidth=".6" />
      </svg>
      {n}
    </span>
  )
}
