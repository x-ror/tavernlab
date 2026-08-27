import { useEffect, useState } from 'react'
import {
  ActionButton,
  Button,
  Flex,
  Item,
  Picker,
  ProgressBar,
  TabList,
  Tabs,
  Text,
  TextArea,
  View,
} from '@adobe/react-spectrum'
import * as api from '../api'
import { go, useApp } from '../store'
import { useT } from '../i18n'
import CardCombo from '../components/CardCombo'
import Gauntlet from './Gauntlet'
import { Caveats, ErrorNote, FieldNote, Loading, Panel, Pill, RateBar, Stat } from '../components/ui'
import ClassCrest from '../components/ClassCrest'
import HeroPortrait from '../components/HeroPortrait'
import { CLASS_KEYS as CLASSES, classColor } from '../classes'
import { useDeckClasses } from '../meta'
import { deckProblem, formatName, pct, signedPct } from '../format'

const TABS = ['rating', 'improve', 'mull', 'opp', 'coach', 'gauntlet']

/* One deck, five questions. The old UI asked for the deck code again on
 * every screen; here it is a single context, and no screen is gated
 * behind another — the simulator answers each of them from scratch in
 * well under a second. */
export default function DeckLab({ tab, sub }) {
  const { t } = useT()
  const { deckCode, deckInfo } = useApp()
  const current = TABS.includes(tab) ? tab : 'rating'

  return (
    <Flex direction="column" gap="size-300">
      <DeckBar />

      <Tabs
        aria-label={t('ui.nav.deck')}
        selectedKey={current}
        onSelectionChange={(k) => go(`deck/${k}`)}
      >
        <TabList>
          <Item key="rating">{t('ui.deck.tab_rating')}</Item>
          <Item key="improve">{t('ui.deck.tab_improve')}</Item>
          <Item key="mull">{t('ui.deck.tab_mull')}</Item>
          <Item key="opp">{t('ui.deck.tab_opp')}</Item>
          <Item key="coach">{t('ui.deck.tab_coach')}</Item>
          <Item key="gauntlet">{t('ui.nav.meta')}</Item>
        </TabList>
      </Tabs>

      {/* The gauntlet is the benchmark, not a property of your deck:
          it reads the same with no deck loaded at all. */}
      {current === 'gauntlet' ? (
        <Gauntlet open={sub ? decodeURIComponent(sub) : null} />
      ) : !deckCode ? null : !deckInfo?.ok && !deckInfo?.pending ? (
        <ErrorNote error={deckProblem(deckInfo, t)} />
      ) : current === 'rating' ? (
        <Rating />
      ) : current === 'improve' ? (
        <Improve />
      ) : current === 'mull' ? (
        <Mulligan />
      ) : current === 'opp' ? (
        <Opponent />
      ) : (
        <CoachNotes />
      )}
    </Flex>
  )
}

function DeckBar() {
  const { t } = useT()
  const { deckCode, setDeckCode, deckInfo, deckName } = useApp()
  const name = deckInfo?.name || deckName
  const title = name || (deckInfo?.cls ? t(`class.${deckInfo.cls}`) : null)
  const [draft, setDraft] = useState(deckCode)
  const [editing, setEditing] = useState(!deckCode)

  useEffect(() => {
    setDraft(deckCode)
    if (deckCode) setEditing(false)
  }, [deckCode])

  if (editing) {
    return (
      <Panel title={t('ui.deck.dialog')}>
        <TextArea
          label={t('deck.label')}
          placeholder={t('deck.ph')}
          value={draft}
          onChange={setDraft}
          width="100%"
          height="size-1200"
        />
        <Flex direction="row" gap="size-150" marginTop="size-200">
          <Button
            variant="accent"
            isDisabled={!draft.trim()}
            onPress={() => setDeckCode(draft.trim())}
          >
            {t('ui.deck.save')}
          </Button>
          {deckCode && (
            <Button variant="secondary" onPress={() => setEditing(false)}>
              {t('ui.deck.cancel')}
            </Button>
          )}
        </Flex>
      </Panel>
    )
  }

  return (
    <Panel style={{ padding: '12px 16px' }}>
      <Flex direction="row" alignItems="center" gap="size-200" wrap>
        <HeroPortrait cls={deckInfo?.cls} size={48} title={deckInfo?.cls} />
        <Flex direction="column" gap="size-25">
          <span
            className="tl-display"
            style={{
              fontSize: '1.05rem',
              fontWeight: 700,
              color: deckInfo?.cls ? classColor(deckInfo.cls) : 'var(--tl-text)',
            }}
          >
            {/* A deck that will not resolve is still a deck the player
                chose. Saying "no deck set" here — as this did while the
                header badge showed the class — reads as data loss. */}
            {deckInfo?.pending ? t('ui.deck.resolving') : title || t('ui.deck.none')}
          </span>
          {deckInfo?.cls && (
            <span style={{ fontSize: '.78rem', color: 'var(--tl-muted)' }}>
              {name ? `${t(`class.${deckInfo.cls}`)} · ` : ''}
              {t('ui.deck.ok', { cls: '', n: deckInfo.total }).replace(/^[,\s]+/, '')} ·{' '}
              {formatName(deckInfo.format, t)}
            </span>
          )}
        </Flex>
        <View flex="1 1 auto" />
        <ActionButton onPress={() => setEditing(true)}>{t('ui.deck.change')}</ActionButton>
      </Flex>
    </Panel>
  )
}

/** Runs `/api/analyze` and hands the result to the shared store. */
function Rating() {
  const { t } = useT()
  const { deckCode, analysis, markAnalysed, deckInfo } = useApp()
  const deckClasses = useDeckClasses(analysis?.format || deckInfo?.format)
  const [games, setGames] = useState('1000')
  const [busy, setBusy] = useState(false)
  const [progress, setProgress] = useState([])
  const [error, setError] = useState(null)

  async function run() {
    setError(null)
    setBusy(true)
    setProgress([])
    try {
      const result = await api.runJob(
        '/api/analyze',
        { code: deckCode, games: Number(games) },
        setProgress,
      )
      markAnalysed(deckCode, result)
    } catch (e) {
      setError(e.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Flex direction="column" gap="size-300">
      <Panel
        title={t('ui.deck.tab_rating')}
        action={
          <Flex direction="row" gap="size-150" alignItems="end">
            <Picker
              label={t('ui.deck.depth')}
              selectedKey={games}
              onSelectionChange={(k) => setGames(String(k))}
              isDisabled={busy}
              width="size-2000"
            >
              <Item key="500">{t('ui.deck.depth_fast')}</Item>
              <Item key="1000">{t('ui.deck.depth_normal')}</Item>
              <Item key="2500">{t('ui.deck.depth_deep')}</Item>
            </Picker>
            <Button variant="accent" isPending={busy} isDisabled={busy} onPress={run}>
              {analysis ? t('ui.deck.rerun') : t('analyze.run')}
            </Button>
          </Flex>
        }
      >
        {busy && (
          <Flex direction="column" gap="size-100">
            <ProgressBar isIndeterminate label={t('common.loading')} width="100%" />
            {progress.length > 0 && <div className="tl-mono">{progress.slice(-4).join('\n')}</div>}
          </Flex>
        )}
        <ErrorNote error={error} />

        {analysis && !busy && (
          <Flex direction="column" gap="size-250">
            <Flex direction="row" gap="size-250" alignItems="center" wrap>
              <HeroPortrait cls={analysis.cls} size={64} title={analysis.cls} />
              <Stat
                label={t('ui.deck.avg')}
                value={pct(analysis.avg, 1)}
                accent={classColor(analysis.cls)}
                tone={analysis.avg >= 0.5 ? 'pos' : 'neg'}
                hint={`${formatName(analysis.format, t)} · ${t('analyze.caption', {
                  games: analysis.games,
                })}`}
              />
            </Flex>
            <View>
              {Object.entries(analysis.rates || {})
                .sort((a, b) => b[1] - a[1])
                .map(([name, v]) => (
                  <RateBar key={name} name={name} value={v} cls={deckClasses[name]} />
                ))}
            </View>
            <FieldNote result={analysis} />
            <Caveats items={[t('analyze.ai_caveat')]} />
          </Flex>
        )}
        {!analysis && !busy && <Text>{t('analyze.caption', { games })}</Text>}
      </Panel>
    </Flex>
  )
}

/** `/api/optimize`: measured swaps only, with the delta that bought them. */
function Improve() {
  const { t } = useT()
  const { deckCode } = useApp()
  const [busy, setBusy] = useState(false)
  const [progress, setProgress] = useState([])
  const [result, setResult] = useState(null)
  const [error, setError] = useState(null)

  async function run() {
    setError(null)
    setBusy(true)
    setProgress([])
    setResult(null)
    try {
      setResult(await api.runJob('/api/optimize', { code: deckCode }, setProgress))
    } catch (e) {
      setError(e.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel
      title={t('ui.deck.tab_improve')}
      action={
        <Button variant="accent" isPending={busy} isDisabled={busy} onPress={run}>
          {t('analyze.optimize')}
        </Button>
      }
    >
      {busy && (
        <Flex direction="column" gap="size-100">
          <ProgressBar isIndeterminate label={t('common.loading')} width="100%" />
          {progress.length > 0 && <div className="tl-mono">{progress.slice(-6).join('\n')}</div>}
        </Flex>
      )}
      <ErrorNote error={error} />
      {result && (
        <Flex direction="column" gap="size-200">
          <Text UNSAFE_style={{ fontWeight: 600 }}>
            {t('optimize.new_avg', { v: (result.new_avg * 100).toFixed(1) })}
          </Text>
          {result.swaps?.length ? (
            <Flex direction="column" gap="size-100">
              {result.swaps.map(([out, inn, delta], i) => (
                <Swap key={i} out={out} inn={inn} delta={delta} />
              ))}
            </Flex>
          ) : (
            <Text>{t('optimize.none')}</Text>
          )}
          {result.near?.length > 0 && (
            <View marginTop="size-150">
              <Text UNSAFE_style={{ fontWeight: 600 }}>{t('optimize.near')}</Text>
              <Flex direction="column" gap="size-75" marginTop="size-100">
                {result.near.map(([out, inn, delta], i) => (
                  <Swap key={i} out={out} inn={inn} delta={delta} muted />
                ))}
              </Flex>
            </View>
          )}
          <FieldNote result={result} />
          <Caveats
            items={[
              t('optimize.confirmed', { games: result.confirm_games }),
              t('analyze.ai_caveat'),
            ]}
          />
        </Flex>
      )}
    </Panel>
  )
}

function Swap({ out, inn, delta, muted }) {
  return (
    <Flex
      direction="row"
      alignItems="center"
      gap="size-150"
      UNSAFE_style={{ opacity: muted ? 0.7 : 1 }}
    >
      <Text UNSAFE_style={{ textDecoration: 'line-through', opacity: 0.7 }}>{out}</Text>
      <Text>→</Text>
      <Text UNSAFE_style={{ fontWeight: 600 }}>{inn}</Text>
      <Pill tone={delta > 0 ? 'pos' : 'neutral'}>{signedPct(delta)}</Pill>
    </Flex>
  )
}

function Mulligan() {
  const { t } = useT()
  const { deckCode } = useApp()
  const [opp, setOpp] = useState('DRUID')
  const [hand, setHand] = useState([])
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState(null)
  const [error, setError] = useState(null)

  async function run() {
    setError(null)
    setBusy(true)
    try {
      setResult(await api.post('/api/mull', { code: deckCode, opp, hand }))
    } catch (e) {
      setError(e.message)
      setResult(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel title={t('ui.deck.tab_mull')}>
      <Flex direction="row" gap="size-200" alignItems="end" wrap>
        <ClassPicker label={t('mull.opp_class')} value={opp} onChange={setOpp} />
        <View flex="1 1 size-3000" minWidth="size-3000">
          <CardCombo label={t('mull.hand_label')} onAdd={(name) => setHand((h) => [...h, name])} />
        </View>
        <Button variant="accent" isDisabled={busy || !hand.length} isPending={busy} onPress={run}>
          {t('mull.run')}
        </Button>
      </Flex>

      <Flex direction="row" gap="size-100" wrap marginTop="size-200">
        {hand.length === 0 && <Text UNSAFE_style={{ opacity: 0.7 }}>{t('ui.mull.empty')}</Text>}
        {hand.map((name, i) => (
          <span key={`${name}-${i}`} className="tl-chip">
            {name}
            <ActionButton
              isQuiet
              aria-label={t('ui.common.close')}
              onPress={() => setHand((h) => h.filter((_, j) => j !== i))}
            >
              ✕
            </ActionButton>
          </span>
        ))}
      </Flex>

      <ErrorNote error={error} />
      {busy && <Loading />}

      {result && !busy && (
        <Flex direction="column" gap="size-150" marginTop="size-250">
          <Text UNSAFE_style={{ opacity: 0.8 }}>
            {t('mull.vs', { deck: result.opp_deck, base: Math.round((result.base || 0) * 100) })}{' '}
            {t('mull.sample', { games: result.games })}
          </Text>
          {result.cards.map((c) => (
            <Flex key={c.card} direction="row" gap="size-150" alignItems="center" wrap>
              <Pill tone={c.keep ? 'pos' : 'neg'}>{c.keep ? t('mull.keep') : t('mull.toss')}</Pill>
              <Text UNSAFE_style={{ fontWeight: 600, minWidth: '10rem' }}>
                ({c.cost}) {c.card}
              </Text>
              <Text UNSAFE_style={{ fontVariantNumeric: 'tabular-nums', minWidth: '4rem' }}>
                {c.delta === null ? '—' : signedPct(c.delta)}
              </Text>
              <Text UNSAFE_style={{ fontSize: '.85rem', opacity: 0.8, flex: '1 1 14rem' }}>
                {why(c.why, t)}
              </Text>
            </Flex>
          ))}
          <Caveats items={[t('coach.note')]} />
        </Flex>
      )}
    </Panel>
  )
}

/* The server answers with reason keys rather than a sentence: it serves a
 * bilingual UI and cannot know which language the screen is in. */
function why(reasons, t) {
  return (reasons || [])
    .map(({ k, n }) =>
      t(`ui.mull.why_${k}`, { n: k === 'measured' ? signedPct(n) : n }),
    )
    .join('; ')
}

function Opponent() {
  const { t } = useT()
  const { deckInfo, analysis } = useApp()
  const [opp, setOpp] = useState('ROGUE')
  const [seen, setSeen] = useState([])
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState(null)
  const [error, setError] = useState(null)
  const format = analysis?.format || deckInfo?.format || 'standard'

  async function run() {
    setError(null)
    setBusy(true)
    try {
      setResult(await api.post('/api/predict', { opp, seen, format }))
    } catch (e) {
      setError(e.message)
      setResult(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel title={t('ui.deck.tab_opp')}>
      <Flex direction="row" gap="size-200" alignItems="end" wrap>
        <ClassPicker label={t('opp.class_label')} value={opp} onChange={setOpp} />
        <View flex="1 1 size-3000" minWidth="size-3000">
          <CardCombo label={t('opp.seen_label')} onAdd={(name) => setSeen((s) => [...s, name])} />
        </View>
        <Button variant="accent" isDisabled={busy} isPending={busy} onPress={run}>
          {t('opp.run')}
        </Button>
      </Flex>

      <Flex direction="row" gap="size-100" wrap marginTop="size-200">
        {seen.map((name, i) => (
          <span key={`${name}-${i}`} className="tl-chip">
            {name}
            <ActionButton
              isQuiet
              aria-label={t('ui.common.close')}
              onPress={() => setSeen((s) => s.filter((_, j) => j !== i))}
            >
              ✕
            </ActionButton>
          </span>
        ))}
      </Flex>

      <ErrorNote error={error} />
      {busy && <Loading />}

      {result && !busy && (
        <Flex direction="column" gap="size-250" marginTop="size-250">
          {result.decks
            .slice()
            .sort((a, b) => b.frac - a.frac)
            .map((d) => (
              <View key={d.deck}>
                <Flex direction="row" gap="size-150" alignItems="center" wrap>
                  <Text UNSAFE_style={{ fontWeight: 600 }}>{d.deck}</Text>
                  <Text UNSAFE_style={{ opacity: 0.75 }}>
                    {t('opp.match', {
                      hits: d.hits,
                      seen: d.seen,
                      frac: Math.round(d.frac * 100),
                    })}
                  </Text>
                </Flex>
                {d.threats.length > 0 && (
                  <Flex direction="column" gap="size-50" marginTop="size-100" marginStart="size-200">
                    <Text UNSAFE_style={{ fontSize: '.8rem', opacity: 0.75 }}>{t('opp.expect')}</Text>
                    {d.threats.map((th) => (
                      <Text key={th.card} UNSAFE_style={{ fontSize: '.85rem' }}>
                        ({th.cost}) <b>{th.card}</b> — {th.text}
                      </Text>
                    ))}
                  </Flex>
                )}
              </View>
            ))}
        </Flex>
      )}
    </Panel>
  )
}

/** `/api/coach`: the matchups that hurt, and the cards the simulations
 *  like and dislike in *your* list. Computed on demand — nothing has to
 *  be run before this screen works. */
function CoachNotes() {
  const { t } = useT()
  const { deckCode } = useApp()
  const [coach, setCoach] = useState(null)
  const [error, setError] = useState(null)
  const deckClasses = useDeckClasses(coach?.format)

  useEffect(() => {
    let live = true
    setCoach(null)
    setError(null)
    api
      .post('/api/coach', { code: deckCode })
      .then((d) => live && setCoach(d))
      .catch((e) => live && setError(e.message))
    return () => {
      live = false
    }
  }, [deckCode])

  if (error) return <ErrorNote error={error} />
  if (!coach) return <Loading />

  const column = (title, rows, colour) => (
    <View flex="1 1 50%">
      <Text UNSAFE_style={{ fontWeight: 600, color: colour }}>{title}</Text>
      <Flex direction="column" gap="size-50" marginTop="size-100">
        {rows.length === 0 && <Text UNSAFE_style={{ opacity: 0.7 }}>{t('coach.empty')}</Text>}
        {rows.map(([name, d, n]) => (
          <Flex key={name} direction="row" justifyContent="space-between" gap="size-150">
            <Text>{name}</Text>
            <Text UNSAFE_style={{ fontVariantNumeric: 'tabular-nums' }}>
              {signedPct(d)}
              <span style={{ opacity: 0.55, fontSize: '.78rem' }}> · {n}</span>
            </Text>
          </Flex>
        ))}
      </Flex>
    </View>
  )

  return (
    <Flex direction="column" gap="size-300">
      <Panel title={t('ui.deck.tab_coach')}>
        <Text>{t('coach.intro')}</Text>
        <View marginTop="size-250">
          <Text UNSAFE_style={{ fontWeight: 600 }}>{t('coach.weak')}</Text>
          <Flex direction="column" gap="size-75" marginTop="size-100">
            {coach.weak.map(([name, v]) => (
              <RateBar key={name} name={name} value={v} cls={deckClasses[name]} />
            ))}
          </Flex>
        </View>

        <Flex direction={{ base: 'column', M: 'row' }} gap="size-400" marginTop="size-300">
          {column(t('coach.keep'), coach.keep, 'var(--tl-pos)')}
          {column(t('coach.cut'), coach.cut, 'var(--tl-neg)')}
        </Flex>

        <Caveats
          items={[t('coach.sample', { games: coach.games, n: coach.min_n }), t('coach.note')]}
        />
      </Panel>
    </Flex>
  )
}

function ClassPicker({ label, value, onChange }) {
  const { t } = useT()
  return (
    <Picker
      label={label}
      items={CLASSES.map((c) => ({ id: c, name: t(`class.${c}`) }))}
      selectedKey={value}
      onSelectionChange={(k) => onChange(String(k))}
      width="size-2400"
    >
      {(item) => (
        <Item textValue={item.name}>
          <Flex direction="row" alignItems="center" gap="size-100">
            <ClassCrest cls={item.id} size={15} />
            <Text>{item.name}</Text>
          </Flex>
        </Item>
      )}
    </Picker>
  )
}
