import { useState } from 'react'
import { tileArt } from '../classes'
import { useT } from '../i18n'

/* A minion or a card in hand, drawn the way Hearthstone draws it: the
 * strip of card art behind a dark scrim, a mana gem on the left, attack
 * and health on the corners.
 *
 * `card_id` is missing for anything the log kept hidden. That case gets
 * the same frame with no art and an explicit "hidden card" label — the
 * shape of the board stays readable even when its contents are not.
 */
const num = (v) => (typeof v === 'number' ? v : 0)

export default function CardTile({ entity, kind = 'minion', count = 0 }) {
  const { t } = useT()
  const [failed, setFailed] = useState(false)
  const e = entity || {}
  const art = e.card_id && !failed ? tileArt(e.card_id) : null

  const hp = num(e.health) - num(e.damage)
  const damaged = num(e.damage) > 0
  const dead = num(e.health) > 0 && hp <= 0
  const tags = e.tags || {}

  const marks = [
    tags.TAUNT && 'taunt',
    tags.DIVINE_SHIELD && 'shield',
    tags.STEALTH && 'stealth',
    tags.FROZEN && 'frozen',
  ].filter(Boolean)

  return (
    <div
      className={[
        'tl-tile',
        `tl-tile-${kind}`,
        tags.TAUNT ? 'is-taunt' : '',
        tags.DIVINE_SHIELD ? 'is-shield' : '',
        dead ? 'is-dead' : '',
        count > 1 ? 'is-counted' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      title={e.name || t('replay.card_unknown')}
    >
      {art && (
        <img className="tl-tile-art" src={art} alt="" onError={() => setFailed(true)} />
      )}
      <span className="tl-tile-scrim" aria-hidden="true" />

      {/* A minion on the board has no cost printed on it in the game
          either; the gem belongs to the card in hand. */}
      {kind === 'hand' && (
        <span className="tl-gem" aria-label={t('replay.mana', { n: num(e.cost), of: '' })}>
          {num(e.cost)}
        </span>
      )}

      <span className="tl-tile-name">{e.name || t('replay.card_unknown')}</span>

      {kind === 'minion' && (
        <>
          <span className="tl-cs tl-atk">{num(e.atk)}</span>
          <span className={`tl-cs tl-hp${damaged ? ' is-hurt' : ''}`}>{hp}</span>
        </>
      )}

      {count > 1 && <span className="tl-count">×{count}</span>}

      {marks.length > 0 && (
        <span className="tl-tile-marks" aria-hidden="true">
          {marks.map((m) => (
            <i key={m} className={`tl-mark tl-mark-${m}`} />
          ))}
        </span>
      )}
    </div>
  )
}
