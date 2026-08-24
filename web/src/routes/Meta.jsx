import { useCallback, useEffect, useState } from 'react'
import { Button, Flex, Item, Picker, ProgressBar, Text } from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { useT } from '../i18n'
import HeroPortrait from '../components/HeroPortrait'
import { Caveats, ErrorNote, Loading, Panel } from '../components/ui'
import { classColor, classWash } from '../classes'
import { formatName, pct } from '../format'

/* A tier list the simulator can defend.
 *
 * Not the ladder: TavernLab does not scrape HSReplay or Untapped for
 * winrates (design U24), so the honest thing it *can* do is play the
 * gauntlet against itself and say so plainly. The three caveats under
 * the table are not decoration — a scripted AI systematically underrates
 * combo, and twelve decks are not a ladder.
 *
 * The matrix is quadratic, so it never runs on load. It is a job the
 * player starts, cached on disk afterwards.
 */
const TIER_COLOR = {
  S: '#e8c65a',
  A: '#4cbf8a',
  B: '#6fb3d8',
  C: '#a2968a',
  D: '#e0525a',
}

export default function Meta() {
  const { t, lang } = useT()
  const { analysis, deckInfo } = useApp()
  const [fmt, setFmt] = useState(analysis?.format || deckInfo?.format || 'standard')
  const [data, setData] = useState(undefined) // undefined = loading
  const [games, setGames] = useState('200')
  const [busy, setBusy] = useState(false)
  const [progress, setProgress] = useState([])
  const [error, setError] = useState(null)

  const load = useCallback(async (which) => {
    setData(undefined)
    try {
      const d = await api.get(`/api/tiers?format=${encodeURIComponent(which)}`)
      setData(d?.decks ? d : null)
    } catch {
      setData(null)
    }
  }, [])

  useEffect(() => {
    load(fmt)
  }, [fmt, load])

  async function run() {
    setError(null)
    setBusy(true)
    setProgress([])
    try {
      const result = await api.runJob(
        '/api/tiers',
        { format: fmt, games: Number(games) },
        setProgress,
      )
      setData(result)
    } catch (e) {
      setError(e.message)
    } finally {
      setBusy(false)
    }
  }

  // The noise caveat is conditional and comes first when it fires: at a
  // low games-per-pair the matchup numbers are wider than the tier
  // bands, which matters more than anything else on the page.
  const marginPts = data?.margin ? Math.round(data.margin * 1000) / 10 : null
  const caveats = [
    marginPts !== null && data.margin > 0.05
      ? t('meta.caveat_noisy', { games: data.games_per_pair, margin: marginPts })
      : null,
    t('meta.caveat_not_ladder'),
    t('meta.caveat_scripted_ai'),
    t('meta.caveat_small_field'),
  ].filter(Boolean)

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
        <Picker
          label={t('ui.tiers.depth')}
          selectedKey={games}
          onSelectionChange={(k) => setGames(String(k))}
          isDisabled={busy}
          width="size-1600"
        >
          <Item key="50">50</Item>
          <Item key="200">200</Item>
          <Item key="500">500</Item>
        </Picker>
        <span style={{ flex: '1 1 auto' }} />
        <Button variant="accent" isPending={busy} isDisabled={busy} onPress={run}>
          {data ? t('ui.tiers.rerun') : t('ui.tiers.run')}
        </Button>
      </Flex>

      <Panel title={t('ui.tiers.title')}>
        <Text UNSAFE_style={{ fontSize: '.86rem', color: 'var(--tl-muted)' }}>
          {t('ui.tiers.intro')} {t('ui.tiers.bands')}
        </Text>
        {data && (
          <Text
            UNSAFE_style={{
              display: 'block',
              marginTop: '.4rem',
              fontSize: '.78rem',
              color: 'var(--tl-faint)',
            }}
          >
            {t('ui.tiers.computed', {
              when: data.computed_at
                ? new Date(data.computed_at * 1000).toLocaleString(
                    lang === 'uk' ? 'uk-UA' : 'en-US',
                  )
                : '—',
              games: data.games_per_pair ?? '—',
            })}
            {marginPts !== null ? ` · ±${marginPts}` : ''}
          </Text>
        )}
        <Caveats items={caveats} />
      </Panel>

      {busy && (
        <Panel>
          <ProgressBar isIndeterminate label={t('ui.tiers.working')} width="100%" />
          {progress.length > 0 && (
            <div className="tl-mono" style={{ marginTop: '.6rem' }}>
              {progress.slice(-4).join('\n')}
            </div>
          )}
        </Panel>
      )}

      <ErrorNote error={error} />

      {data === undefined ? (
        <Loading />
      ) : data === null ? (
        !busy && (
          <Panel>
            <Text>
              {t('ui.tiers.none', { pairs: 12 * 11, games })}
            </Text>
          </Panel>
        )
      ) : (
        <Flex direction="column" gap="size-200">
          {data.decks.map((deck, i) => (
            <TierRow key={deck.name} deck={deck} rank={i + 1} />
          ))}
        </Flex>
      )}
    </Flex>
  )
}

function TierRow({ deck, rank }) {
  const { t } = useT()
  const tone = classColor(deck.cls)
  const tier = TIER_COLOR[deck.tier] || TIER_COLOR.C
  const sorted = Object.entries(deck.vs || {}).sort((a, b) => b[1] - a[1])
  const best = sorted.slice(0, 2)
  const worst = sorted.slice(-2).reverse()

  return (
    <div
      className="tl-panel"
      style={{
        padding: '14px 18px',
        background: `linear-gradient(100deg, ${classWash(deck.cls, 0.14)}, var(--tl-surface) 60%)`,
      }}
    >
      <Flex direction="row" alignItems="center" gap="size-250" wrap>
        <div
          className="tl-display"
          style={{
            width: 44,
            height: 44,
            flex: '0 0 auto',
            display: 'grid',
            placeItems: 'center',
            borderRadius: 8,
            fontSize: '1.5rem',
            fontWeight: 700,
            color: tier,
            border: `2px solid ${tier}`,
            background: 'rgba(0,0,0,.35)',
          }}
          title={`#${rank}`}
        >
          {deck.tier}
        </div>

        <HeroPortrait cls={deck.cls} size={44} title={deck.cls} />

        <Flex direction="column" gap="size-25" flex="1 1 12rem" minWidth="size-2400">
          <span className="tl-display" style={{ fontSize: '1.05rem', fontWeight: 700, color: tone }}>
            {deck.name}
          </span>
          <span style={{ fontSize: '.76rem', color: 'var(--tl-muted)' }}>
            {t(`class.${deck.cls}`)}
            {deck.archetype ? ` · ${deck.archetype}` : ''}
          </span>
        </Flex>

        <Matchups label={t('ui.tiers.vs_best')} rows={best} />
        <Matchups label={t('ui.tiers.vs_worst')} rows={worst} />

        <Flex direction="column" alignItems="center" minWidth="size-1250">
          <span
            style={{
              fontSize: '.68rem',
              letterSpacing: '.1em',
              textTransform: 'uppercase',
              color: 'var(--tl-muted)',
            }}
          >
            {t('ui.tiers.winrate')}
          </span>
          <span
            className="tl-display"
            style={{
              fontSize: '1.5rem',
              fontWeight: 700,
              color: deck.winrate >= 0.5 ? 'var(--tl-pos)' : 'var(--tl-neg)',
            }}
          >
            {pct(deck.winrate, 1)}
          </span>
        </Flex>
      </Flex>
    </div>
  )
}

function Matchups({ label, rows }) {
  if (!rows.length) return null
  return (
    <Flex direction="column" gap="size-25" minWidth="size-2000" UNSAFE_style={{ flex: '0 1 auto' }}>
      <span style={{ fontSize: '.66rem', letterSpacing: '.08em', textTransform: 'uppercase', color: 'var(--tl-faint)' }}>
        {label}
      </span>
      {rows.map(([name, rate]) => (
        <span key={name} style={{ fontSize: '.74rem', color: 'var(--tl-muted)' }}>
          <span
            style={{
              color: rate >= 0.5 ? 'var(--tl-pos)' : 'var(--tl-neg)',
              fontVariantNumeric: 'tabular-nums',
              fontWeight: 600,
            }}
          >
            {pct(rate)}
          </span>{' '}
          {name}
        </span>
      ))}
    </Flex>
  )
}
