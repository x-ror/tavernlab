import { Content, Flex, Heading, InlineAlert, ProgressCircle, Text } from '@adobe/react-spectrum'
import { useT } from '../i18n'
import { classColor } from '../classes'
import ClassCrest from './ClassCrest'

/** A titled panel. The page rhythm comes from these and the spacing
 *  between them, not from rules and boxes inside them. */
export function Panel({ title, action, children, className = '', style }) {
  return (
    <section className={`tl-panel ${className}`} style={{ padding: '18px 20px', ...style }}>
      {(title || action) && (
        <Flex
          direction="row"
          alignItems="center"
          justifyContent="space-between"
          gap="size-150"
          marginBottom="size-200"
          wrap
        >
          {title ? <h3 className="tl-panel-title">{title}</h3> : <span />}
          {action}
        </Flex>
      )}
      {children}
    </section>
  )
}

export function Loading({ label }) {
  const { t } = useT()
  return (
    <Flex alignItems="center" gap="size-150" marginY="size-200">
      <ProgressCircle size="S" isIndeterminate aria-label={label || t('common.loading')} />
      <Text>{label || t('common.loading')}</Text>
    </Flex>
  )
}

export function ErrorNote({ error, children }) {
  const { t } = useT()
  if (!error) return null
  return (
    <InlineAlert variant="negative" width="100%" marginY="size-150">
      <Heading>{t('ui.common.error')}</Heading>
      <Content>
        {String(error)}
        {children}
      </Content>
    </InlineAlert>
  )
}

/** Caveats are first-class in this product, not small grey print. */
export function Caveats({ items, title }) {
  const { t } = useT()
  const list = (items || []).filter(Boolean)
  if (!list.length) return null
  return (
    <InlineAlert variant="notice" width="100%" marginTop="size-200">
      <Heading>{title || t('review.caveats')}</Heading>
      <Content>
        <ul style={{ margin: 0, paddingInlineStart: '1.1em' }}>
          {list.map((c, i) => (
            <li key={i}>
              <Text>{c}</Text>
            </li>
          ))}
        </ul>
      </Content>
    </InlineAlert>
  )
}

export function Stat({ label, value, hint, tone, accent }) {
  return (
    <div className="tl-stat" style={{ flex: '1 1 220px', minWidth: 170 }}>
      {accent && (
        <span
          aria-hidden="true"
          style={{
            position: 'absolute',
            inset: 'auto auto 0 0',
            width: '100%',
            height: 2,
            background: `linear-gradient(90deg, ${accent}, transparent)`,
          }}
        />
      )}
      <div className="tl-stat-label">{label}</div>
      <div
        className="tl-stat-value"
        style={{
          color:
            tone === 'pos' ? 'var(--tl-pos)' : tone === 'neg' ? 'var(--tl-neg)' : 'var(--tl-text)',
        }}
      >
        {value}
      </div>
      {hint && <div className="tl-stat-hint">{hint}</div>}
    </div>
  )
}

/** A matchup bar: the opponent's crest, the deck, the measured rate.
 *  Filled in the class colour, so a column of them reads as a spectrum
 *  of opponents rather than as eleven identical grey bars. */
export function RateBar({ name, value, cls }) {
  const pct = Math.round((value || 0) * 100)
  const tone = cls ? classColor(cls) : pct >= 50 ? 'var(--tl-pos)' : 'var(--tl-neg)'
  return (
    <Flex direction="row" alignItems="center" gap="size-150" marginBottom="size-100">
      {cls && <ClassCrest cls={cls} size={16} />}
      <Text UNSAFE_style={{ flex: '0 0 auto', minWidth: '10.5rem', fontSize: '.88rem' }}>
        {name}
      </Text>
      <div
        style={{
          flex: '1 1 auto',
          height: 8,
          borderRadius: 4,
          background: 'rgba(0,0,0,.45)',
          boxShadow: 'inset 0 1px 2px rgba(0,0,0,.6)',
          overflow: 'hidden',
          minWidth: 70,
        }}
      >
        <div
          style={{
            width: `${pct}%`,
            height: '100%',
            borderRadius: 4,
            background: `linear-gradient(90deg, ${tone}88, ${tone})`,
          }}
        />
      </div>
      <Text
        UNSAFE_style={{
          flex: '0 0 3rem',
          textAlign: 'right',
          fontVariantNumeric: 'tabular-nums',
          color: pct >= 50 ? 'var(--tl-pos)' : 'var(--tl-neg)',
          fontWeight: 600,
        }}
      >
        {pct}%
      </Text>
    </Flex>
  )
}

/** A quiet pill. Spectrum's Badge reads as an alert; a keep/toss verdict
 *  or a measured delta wants to be legible, not loud. */
export function Pill({ tone, children }) {
  const map = {
    pos: ['rgba(76,191,138,.16)', 'var(--tl-pos)'],
    neg: ['rgba(224,82,90,.16)', 'var(--tl-neg)'],
    warn: ['rgba(232,163,61,.16)', 'var(--tl-warn)'],
  }
  const [bg, fg] = map[tone] || ['rgba(255,255,255,.07)', 'var(--tl-muted)']
  return (
    <span
      style={{
        display: 'inline-block',
        padding: '2px 10px',
        borderRadius: 999,
        background: bg,
        color: fg,
        border: `1px solid ${fg}44`,
        fontSize: '.78rem',
        fontWeight: 600,
        whiteSpace: 'nowrap',
      }}
    >
      {children}
    </span>
  )
}

/** Which gauntlet decks the answer above could not include, and why.
 *
 *  Every rate on these screens is an average over the field. When a deck
 *  in that field holds a card the engine cannot play it is left out of
 *  the average entirely, and an average over seven decks presented as an
 *  average over twelve is a lie by omission — so the server reports the
 *  gap on every such answer and this prints it. */
export function FieldNote({ result }) {
  const { t } = useT()
  if (!result || result.field_played === undefined) return null
  const skipped = result.field_skipped || []
  if (!skipped.length) return null
  return (
    <InlineAlert variant="notice" width="100%">
      <Heading>{t('ui.field.title', { played: result.field_played, all: result.field_decks })}</Heading>
      <Content>
        <ul style={{ margin: 0, paddingInlineStart: '1.1em' }}>
          {skipped.map((d) => (
            <li key={d.deck}>
              <Text>
                {d.deck} — {(d.cards || []).map(([name, n]) => `${n}× ${name}`).join(', ')}
              </Text>
            </li>
          ))}
        </ul>
        <Text UNSAFE_style={{ display: 'block', marginTop: '.4rem', fontSize: '.85rem', opacity: 0.8 }}>
          {t('ui.field.why')}
        </Text>
      </Content>
    </InlineAlert>
  )
}
