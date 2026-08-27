import { useMemo, useState } from 'react'
import {
  ActionButton,
  Flex,
  Item,
  Picker,
  Text,
  Tooltip,
  TooltipTrigger,
} from '@adobe/react-spectrum'
import { go, useApp } from '../store'
import { useT } from '../i18n'
import { manaCurve, useMetaDecks } from '../meta'
import HeroPortrait from '../components/HeroPortrait'
import CardTile from '../components/CardTile'
import { Loading, Panel } from '../components/ui'
import { classColor, classWash } from '../classes'
import { formatName, pct } from '../format'

/* The gauntlet, shown rather than described.
 *
 * Every rating in this app is "against these twelve decks"; until now
 * the player could read the number without ever seeing what produced
 * it. Their own win rate against each is joined in when they have one,
 * and the list is then sorted worst-first — that is the coaching order,
 * not the alphabet.
 *
 * This is deliberately **not** called the meta. A meta is a ladder tier
 * list with each deck's own win rate, and TavernLab cannot have one:
 * design U24 rules out scraping HSReplay/Untapped for winrates. Naming
 * a fixed benchmark "Meta" would promise exactly the thing the project
 * has decided not to ship.
 */
export default function Meta({ open }) {
  const { t } = useT()
  const { analysis, deckInfo } = useApp()
  const [fmt, setFmt] = useState(analysis?.format || deckInfo?.format || 'standard')
  const decks = useMetaDecks(fmt)

  const rates = analysis?.format === fmt ? analysis.rates || {} : {}
  const hasRates = Object.keys(rates).length > 0

  const ordered = useMemo(() => {
    if (!decks) return null
    const list = [...decks]
    if (hasRates) {
      list.sort((a, b) => (rates[a.name] ?? 9) - (rates[b.name] ?? 9))
    } else {
      list.sort((a, b) => a.cls.localeCompare(b.cls) || a.name.localeCompare(b.name))
    }
    return list
  }, [decks, hasRates, rates])

  return (
    <Flex direction="column" gap="size-300">
      <Flex direction="row" gap="size-200" alignItems="end" wrap>
        <Picker
          label={t('ui.meta.format')}
          items={[
            { id: 'standard', name: formatName('standard', t) },
            { id: 'wild', name: formatName('wild', t) },
          ]}
          selectedKey={fmt}
          onSelectionChange={(k) => setFmt(String(k))}
          width="size-2000"
        >
          {(item) => <Item>{item.name}</Item>}
        </Picker>
        <span style={{ flex: '1 1 auto' }} />
        {ordered && (
          <Text UNSAFE_style={{ color: 'var(--tl-muted)', paddingBottom: 6 }}>
            {t('ui.meta.count', {
              n: ordered.length,
              playable: ordered.filter((d) => d.playable !== false).length,
            })}
          </Text>
        )}
      </Flex>

      <Panel style={{ padding: '14px 18px' }}>
        <Text UNSAFE_style={{ fontSize: '.86rem', color: 'var(--tl-muted)' }}>
          {t('ui.meta.intro')}
          {hasRates ? ` ${t('ui.meta.sorted_by_wr')}` : ` ${t('ui.meta.no_wr')}`}
        </Text>
        <Text
          UNSAFE_style={{
            display: 'block',
            marginTop: '.5rem',
            fontSize: '.78rem',
            color: 'var(--tl-faint)',
          }}
        >
          {t('ui.meta.not_meta')}
        </Text>
      </Panel>

      {!ordered ? (
        <Loading />
      ) : ordered.length === 0 ? (
        <Panel>
          <Text>{t('ui.meta.empty')}</Text>
        </Panel>
      ) : (
        <Flex direction="column" gap="size-250">
          {ordered.map((deck) => (
            <DeckCard
              key={deck.name}
              deck={deck}
              rate={rates[deck.name]}
              open={open === deck.name}
            />
          ))}
        </Flex>
      )}
    </Flex>
  )
}

function DeckCard({ deck, rate, open }) {
  const { t } = useT()
  const { setDeckCode } = useApp()
  const [copied, setCopied] = useState(null)
  const toggle = () =>
    go(open ? 'deck/gauntlet' : `deck/gauntlet/${encodeURIComponent(deck.name)}`)

  /* The same block a deck site hands out, and the same one this app
     parses back — so a copied list round-trips through the paste box. */
  const asText = () =>
    [
      `### ${deck.name}`,
      `# ${t(`class.${deck.cls}`)}`,
      '#',
      ...(deck.cardlist || []).map((c) => `# ${c.n}x (${c.cost}) ${c.card}`),
      '#',
      deck.deckstring || '',
    ]
      .filter(Boolean)
      .join('\n')

  async function copy(what, kind) {
    try {
      await navigator.clipboard.writeText(what)
      setCopied(kind)
      setTimeout(() => setCopied(null), 1600)
    } catch {
      /* clipboard blocked: nothing to fall back to, and a thrown
         exception here would take the page down with it */
    }
  }
  const tone = classColor(deck.cls)
  const cards = deck.cardlist || []
  const total = cards.reduce((n, c) => n + (c.n || 1), 0)

  return (
    <div
      className="tl-panel"
      style={{
        padding: '16px 18px',
        background: `linear-gradient(100deg, ${classWash(deck.cls, 0.16)}, var(--tl-surface) 62%)`,
      }}
    >
      <Flex direction="row" alignItems="center" gap="size-250" wrap>
        <HeroPortrait cls={deck.cls} size={54} title={deck.cls} />

        <Flex direction="column" gap="size-25" flex="1 1 14rem" minWidth="size-3000">
          <span className="tl-display" style={{ fontSize: '1.1rem', fontWeight: 700, color: tone }}>
            {deck.name}
          </span>
          <span style={{ fontSize: '.78rem', color: 'var(--tl-muted)' }}>
            {t(`class.${deck.cls}`)}
            {deck.archetype ? ` · ${deck.archetype}` : ''} · {total}
            {deck.deckstring && !deck.deckstring_complete && (
              <span style={{ color: 'var(--tl-warn)' }}>
                {' · '}
                {t('ui.meta.code_partial')}
              </span>
            )}
          </span>
          {/* A deck the engine cannot field is not scored against, and
              saying so here is the only place the player can find out
              why their rating averaged over fewer decks. */}
          {deck.playable === false && (
            <span style={{ fontSize: '.78rem', color: 'var(--tl-warn)' }}>
              {t('ui.meta.not_fielded', {
                cards: (deck.missing || []).map(([name, n]) => `${n}× ${name}`).join(', '),
              })}
            </span>
          )}
        </Flex>

        <Curve cards={cards} tone={tone} />

        {rate !== undefined && (
          <Flex direction="column" alignItems="center" minWidth="size-1200">
            <span style={{ fontSize: '.7rem', letterSpacing: '.1em', textTransform: 'uppercase', color: 'var(--tl-muted)' }}>
              {t('ui.meta.your_wr')}
            </span>
            <span
              className="tl-display"
              style={{
                fontSize: '1.6rem',
                fontWeight: 700,
                color: rate >= 0.5 ? 'var(--tl-pos)' : 'var(--tl-neg)',
              }}
            >
              {pct(rate)}
            </span>
          </Flex>
        )}

        <Flex direction="row" gap="size-100" alignItems="center" wrap>
          {deck.deckstring ? (
            <ActionButton onPress={() => copy(deck.deckstring, 'code')}>
              {copied === 'code' ? t('ui.meta.copied') : t('ui.meta.copy_code')}
            </ActionButton>
          ) : (
            <TooltipTrigger delay={0}>
              <ActionButton isDisabled>{t('ui.meta.copy_code')}</ActionButton>
              <Tooltip>{t('ui.meta.no_code')}</Tooltip>
            </TooltipTrigger>
          )}
          <ActionButton onPress={() => copy(asText(), 'list')}>
            {copied === 'list' ? t('ui.meta.copied') : t('ui.meta.copy_list')}
          </ActionButton>
          {deck.deckstring && deck.playable !== false && (
            <ActionButton
              onPress={() => {
                setDeckCode(deck.deckstring)
                go('deck/rating')
              }}
            >
              {t('ui.meta.use')}
            </ActionButton>
          )}
          <ActionButton onPress={toggle}>
            {open ? t('ui.meta.hide') : t('ui.meta.show')}
          </ActionButton>
        </Flex>
      </Flex>

      {open && (
        <>
          <hr className="tl-rule" style={{ margin: '14px 0' }} />
          <div className="tl-cardgrid">
            {cards.map((c) => (
              <MetaCard key={c.id} card={c} />
            ))}
          </div>
          {cards.some((c) => !c.implemented) && (
            <Text
              UNSAFE_style={{
                display: 'block',
                marginTop: '.6rem',
                fontSize: '.75rem',
                color: 'var(--tl-faint)',
              }}
            >
              {t('ui.meta.unimplemented')}
            </Text>
          )}
        </>
      )}
    </div>
  )
}

/** A deck's shape at a glance. Eight bars say more than thirty names. */
function Curve({ cards, tone }) {
  const { t } = useT()
  const bins = manaCurve(cards)
  const peak = Math.max(1, ...bins)
  return (
    <Flex
      direction="row"
      alignItems="end"
      gap="size-50"
      height="size-500"
      UNSAFE_style={{ flex: '0 0 auto' }}
      aria-label={t('ui.meta.curve')}
    >
      {bins.map((n, cost) => (
        <div key={cost} style={{ width: 14, textAlign: 'center' }}>
          <div
            title={`${cost}${cost === 7 ? '+' : ''}: ${n}`}
            style={{
              height: Math.round((n / peak) * 34) || 2,
              background: n ? tone : 'rgba(255,255,255,.1)',
              borderRadius: '2px 2px 0 0',
              opacity: n ? 0.85 : 1,
            }}
          />
          <div style={{ fontSize: '.6rem', color: 'var(--tl-faint)', marginTop: 2 }}>
            {cost === 7 ? '7+' : cost}
          </div>
        </div>
      ))}
    </Flex>
  )
}

/** A card in a decklist: the real tile, its cost, and how many copies. */
function MetaCard({ card }) {
  return (
    <div style={{ opacity: card.implemented ? 1 : 0.45 }}>
      <CardTile
        entity={{
          eid: card.id,
          card_id: card.id,
          name: card.card,
          cost: card.cost,
        }}
        kind="hand"
        count={card.n}
      />
    </div>
  )
}
