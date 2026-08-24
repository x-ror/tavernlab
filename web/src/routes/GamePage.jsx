import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ActionButton,
  ActionGroup,
  Badge,
  Button,
  Content,
  Disclosure,
  DisclosurePanel,
  DisclosureTitle,
  Divider,
  Flex,
  Heading,
  InlineAlert,
  Item,
  ProgressBar,
  Slider,
  Text,
  View,
} from '@adobe/react-spectrum'
import * as api from '../api'
import { go, useApp } from '../store'
import { useT } from '../i18n'
import Board from '../components/Board'
import LabelLegend from '../components/LabelLegend'
import WpChart from '../components/WpChart'
import { Caveats, ErrorNote, Loading, Panel, ResultPill } from '../components/ui'
import { Versus } from '../components/HeroPortrait'
import { msgList, msgText, reportParts } from '../msg'
import { fmtDate, matchup } from '../format'

/* Review and replay are two views of one game and one ply, so they live
 * on one page with a shared `seq`. Clicking a key moment or a point on
 * the win-probability line lands on the same position in the replay —
 * in the old UI that was a tab switch that lost your place. */
export default function GamePage({ gameId, view, seq: routeSeq }) {
  const { t, lang } = useT()
  const { refreshGames } = useApp()
  const [game, setGame] = useState(null)
  const [review, setReview] = useState(null)
  const [replay, setReplay] = useState(null)
  const [error, setError] = useState(null)
  const [seq, setSeq] = useState(null)
  const [busy, setBusy] = useState(false)
  const [progress, setProgress] = useState([])

  const load = useCallback(async () => {
    setError(null)
    try {
      const g = await api.games.one(gameId)
      setGame(g.game)
      const [rev, rep] = await Promise.all([
        api.games.review(gameId).catch((e) => ({ error: e.message, status: null })),
        api.games.replay(gameId).catch(() => ({ snapshots: [] })),
      ])
      setReview(rev)
      setReplay(rep.snapshots || [])
    } catch (e) {
      setError(e.message)
    }
  }, [gameId])

  useEffect(() => {
    load()
  }, [load])

  async function analyse(reparse = false) {
    setBusy(true)
    setProgress([])
    try {
      const started = reparse ? await api.games.reparse(gameId) : await api.games.analyze(gameId)
      await api.pollJob(started.job, setProgress)
      await load()
      await refreshGames()
    } catch (e) {
      setError(e.message)
    } finally {
      setBusy(false)
    }
  }

  if (error) return <ErrorNote error={error} />
  if (!game) return <Loading />

  const hasReview = review && review.status && review.report
  const snapshots = replay || []
  // The URL wins: a ply in the hash is what a shared link or a reload
  // has to land on. Local state only fills in before one is chosen.
  const activeSeq = routeSeq ?? seq ?? snapshots[0]?.event_seq ?? null

  return (
    <Flex direction="column" gap="size-300">
      <ActionButton isQuiet onPress={() => go('games')} alignSelf="start">
        ← {t('ui.games.back')}
      </ActionButton>

      <Panel style={{ padding: '16px 20px' }}>
        <Flex direction="row" alignItems="center" gap="size-300" wrap>
          <Versus us={game.player_class} them={game.opponent_class} size={62} />
          <Flex direction="column" gap="size-75" flex="1 1 auto" minWidth="size-3000">
            <Flex direction="row" gap="size-150" alignItems="center" wrap>
              <span className="tl-display" style={{ fontSize: '1.15rem', fontWeight: 700 }}>
                {matchup(game, t)}
              </span>
              <ResultPill result={game.result} label={t(`result.${game.result || 'unknown'}`)} />
            </Flex>
            <Text UNSAFE_style={{ fontSize: '.82rem', color: 'var(--tl-muted)' }}>
              {fmtDate(game.started_at, lang)} · {t('review.turns_n', { n: game.turns ?? '—' })} ·{' '}
              {game.going_first ? t('games.first') : t('games.second')}
            </Text>
          </Flex>
          {game.reviewable !== false && (
            <Button variant="secondary" isDisabled={busy} isPending={busy} onPress={() => analyse(false)}>
              {hasReview ? t('review.reanalyse') : t('review.analyse')}
            </Button>
          )}
        </Flex>
      </Panel>

      {game.reviewable === false && (
        <InlineAlert variant="neutral" width="100%">
          <Heading>{t('review.blocked')}</Heading>
          <Content>{game.review_blocked}</Content>
        </InlineAlert>
      )}

      {busy && (
        <Flex direction="column" gap="size-100">
          <ProgressBar isIndeterminate label={t('ui.game.working')} width="100%" />
          {progress.length > 0 && <div className="tl-mono">{progress.slice(-4).join('\n')}</div>}
        </Flex>
      )}

      <ActionGroup
        selectionMode="single"
        selectedKeys={[view === 'replay' ? 'replay' : 'review']}
        disallowEmptySelection
        onSelectionChange={(keys) => {
          const k = [...keys][0]
          if (k) go(`games/${gameId}/${k}`)
        }}
      >
        <Item key="review">{t('ui.game.review')}</Item>
        <Item key="replay">{t('ui.game.replay')}</Item>
      </ActionGroup>

      {view === 'replay' ? (
        <Replay
          snapshots={snapshots}
          review={review}
          game={game}
          activeSeq={activeSeq}
          onSeq={setSeq}
          gameId={gameId}
        />
      ) : (
        <Review
          review={review}
          activeSeq={activeSeq}
          onSeq={setSeq}
          gameId={gameId}
          onAnalyse={() => analyse(false)}
          busy={busy}
        />
      )}
    </Flex>
  )
}

function Review({ review, activeSeq, onSeq, gameId, onAnalyse, busy }) {
  const { t } = useT()

  if (!review || review.error || !review.report) {
    return (
      <InlineAlert variant="info" width="100%">
        <Heading>{t('ui.game.analyse_hint')}</Heading>
        <Content>
          <Flex direction="column" gap="size-200" alignItems="start">
            <Text>{t('review.not_ready')}</Text>
            <Button variant="accent" isDisabled={busy} onPress={onAnalyse}>
              {t('review.analyse')}
            </Button>
          </Flex>
        </Content>
      </InlineAlert>
    )
  }

  const { report, key_moments: moments = [], wp_series: wp = [], turns = [] } = review
  const { headline, bullets, caveats } = reportParts(report, t)

  return (
    <Flex direction="column" gap="size-300">
      <Panel>
        <Heading level={2} margin={0}>
          {headline}
        </Heading>
        {bullets.length > 0 && (
          <ul style={{ marginTop: '.75rem', paddingInlineStart: '1.1em' }}>
            {bullets.map((b, i) => (
              <li key={i}>
                <Text>{b}</Text>
              </li>
            ))}
          </ul>
        )}
        {review.status === 'partial' && (
          <InlineAlert variant="notice" width="100%" marginTop="size-200">
            <Heading>{t('review_status.partial')}</Heading>
            <Content>{t('review.partial_note')}</Content>
          </InlineAlert>
        )}
      </Panel>

      <Panel title={t('review.key_moments')}>
        {moments.length === 0 ? (
          <Text>{t('review.no_moments')}</Text>
        ) : (
          <Flex direction="column" gap="size-200">
            {moments.map((m, i) => (
              <Flex
                key={i}
                direction="row"
                gap="size-200"
                alignItems="center"
                justifyContent="space-between"
                wrap
              >
                <Flex direction="row" gap="size-150" alignItems="center" flex="1 1 auto" wrap>
                  <Badge variant={m.label === 'missed_lethal' ? 'negative' : 'yellow'}>
                    {t(`label.${m.label}`)}
                  </Badge>
                  <Text UNSAFE_style={{ fontWeight: 600 }}>{t('review.turn_n', { n: m.turn })}</Text>
                  <Text UNSAFE_style={{ flex: '1 1 16rem' }}>
                    {m.details ? msgList(m.details, t).join('; ') : m.detail}
                  </Text>
                  {m.approx && <Badge variant="neutral">{t('review.approx')}</Badge>}
                </Flex>
                <ActionButton
                  onPress={() => go(`games/${gameId}/replay/${m.seq}`)}
                >
                  {t('ui.game.moment_open')}
                </ActionButton>
              </Flex>
            ))}
            {moments.some((m) => m.approx) && <Caveats items={[t('review.approx_note')]} />}
          </Flex>
        )}
      </Panel>

      <Panel title={t('review.wp_title')}>
        <WpChart
          series={wp}
          activeSeq={activeSeq}
          onPick={(p) => go(`games/${gameId}/replay/${p.seq}`)}
        />
        <Text UNSAFE_style={{ fontSize: '.85rem', opacity: 0.8 }}>{t('review.wp_hatched')}</Text>
      </Panel>

      <Panel title={t('ui.game.ledger')}>
        <TurnLedger turns={turns} onSeq={onSeq} gameId={gameId} />
      </Panel>

      <Disclosure>
        <DisclosureTitle>{t('ui.game.legend')}</DisclosureTitle>
        <DisclosurePanel>
          <LabelLegend legend={review.labels_legend} />
        </DisclosurePanel>
      </Disclosure>

      <Caveats items={caveats} />
    </Flex>
  )
}

function TurnLedger({ turns, onSeq, gameId }) {
  const { t } = useT()
  const ours = turns.filter((x) => x.ledger?.side === 'us')
  if (!ours.length) return <Text>{t('review.no_moments')}</Text>

  return (
    <div className="tl-scroll">
      <table style={{ borderCollapse: 'collapse', width: '100%', fontSize: '.9rem' }}>
        <thead>
          <tr style={{ textAlign: 'left', opacity: 0.75 }}>
            <th style={{ padding: '4px 8px' }}>{t('ui.game.turn_col')}</th>
            <th style={{ padding: '4px 8px' }}>{t('ui.game.mana_col')}</th>
            <th style={{ padding: '4px 8px' }}>{t('ui.game.attacks_col')}</th>
            <th style={{ padding: '4px 8px' }}>{t('ui.game.notes_col')}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {ours.map((x) => {
            const led = x.ledger || {}
            const firstSeq = x.decisions?.[0]?.seq
            const leaky = (led.mana_left || 0) > 0 || (led.unused_attacks || 0) > 0
            return (
              <tr key={x.turn} style={{ borderTop: '1px solid var(--tl-grid)' }}>
                <td style={{ padding: '6px 8px', fontVariantNumeric: 'tabular-nums' }}>{x.turn}</td>
                <td
                  style={{
                    padding: '6px 8px',
                    fontVariantNumeric: 'tabular-nums',
                    color: (led.mana_left || 0) > 0 ? 'var(--tl-neg)' : 'inherit',
                  }}
                >
                  {led.mana_left ?? 0}
                </td>
                <td
                  style={{
                    padding: '6px 8px',
                    fontVariantNumeric: 'tabular-nums',
                    color: (led.unused_attacks || 0) > 0 ? 'var(--tl-neg)' : 'inherit',
                  }}
                >
                  {led.unused_attacks ?? 0}
                </td>
                <td style={{ padding: '6px 8px' }}>
                  {(led.notes || []).map((n, i) => (
                    <div key={i}>{msgText(typeof n === 'string' ? n : n.what, t)}</div>
                  ))}
                  {led.lethal && <Badge variant="negative">{t('review.lethal_on_board')}</Badge>}
                  {led.hero_power_skipped && (
                    <Text UNSAFE_style={{ opacity: 0.7 }}>{t('review.hp_skipped')}</Text>
                  )}
                  {!leaky && !led.notes?.length && !led.lethal && !led.hero_power_skipped && (
                    <Text UNSAFE_style={{ opacity: 0.5 }}>—</Text>
                  )}
                </td>
                <td style={{ padding: '6px 8px', textAlign: 'right' }}>
                  {firstSeq !== undefined && (
                    <ActionButton
                      isQuiet
                      onPress={() => go(`games/${gameId}/replay/${firstSeq}`)}
                    >
                      {t('review.open_in_replay')}
                    </ActionButton>
                  )}
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

function Replay({ snapshots, review, game, activeSeq, onSeq, gameId }) {
  const { t } = useT()
  const [playing, setPlaying] = useState(false)
  const timer = useRef(null)

  const index = Math.max(
    0,
    snapshots.findIndex((s) => s.event_seq === activeSeq),
  )
  const snap = snapshots[index]

  const decisions = useMemo(() => {
    const out = new Map()
    for (const turn of review?.turns || []) {
      for (const d of turn.decisions || []) out.set(d.seq, { ...d, turn: turn.turn })
    }
    return out
  }, [review])

  const jump = useCallback(
    (i) => {
      const next = snapshots[Math.min(snapshots.length - 1, Math.max(0, i))]
      if (!next) return
      onSeq(next.event_seq)
      go(`games/${gameId}/replay/${next.event_seq}`)
    },
    [snapshots, onSeq, gameId],
  )

  const step = useCallback((delta) => jump(index + delta), [jump, index])

  useEffect(() => {
    if (!playing) return undefined
    // `step` changes with the index, so the interval is re-armed each
    // tick — simpler than threading a ref through the closure.
    timer.current = setInterval(() => step(1), 900)
    return () => clearInterval(timer.current)
  }, [playing, step, onSeq])

  useEffect(() => {
    if (playing && index >= snapshots.length - 1) setPlaying(false)
  }, [playing, index, snapshots.length])

  if (!snapshots.length) return <Text>{t('replay.no_snapshots')}</Text>

  const decision = decisions.get(snap?.event_seq)
  const visible = snap?.visible

  return (
    <Flex direction="column" gap="size-250">
      <Panel>
        <Flex direction="row" gap="size-100" alignItems="center" wrap>
          <ActionButton onPress={() => jump(0)}>{t('replay.first')}</ActionButton>
          <ActionButton onPress={() => step(-1)}>{t('replay.prev')}</ActionButton>
          <ActionButton onPress={() => setPlaying((p) => !p)}>
            {playing ? t('replay.pause') : t('replay.play')}
          </ActionButton>
          <ActionButton onPress={() => step(1)}>{t('replay.next')}</ActionButton>
          <ActionButton onPress={() => jump(snapshots.length - 1)}>
            {t('replay.last')}
          </ActionButton>
          <View flex="1 1 auto" />
          <Text UNSAFE_style={{ fontVariantNumeric: 'tabular-nums' }}>
            {t('ui.game.ply', { i: index + 1, n: snapshots.length })}
          </Text>
          {typeof snap?.wp === 'number' && (
            <Badge variant="neutral">{t('replay.wp', { v: Math.round(snap.wp * 100) })}</Badge>
          )}
          {!snap?.lethal_ok && <Badge variant="neutral">{t('replay.lethal_off')}</Badge>}
          {!snap?.search_ok && <Badge variant="neutral">{t('replay.search_off')}</Badge>}
        </Flex>

        <Slider
          label={t('review.turn_n', { n: visible?.turn ?? 0 })}
          getValueLabel={() => t('replay.ply', { seq: snap?.event_seq ?? 0 })}
          minValue={0}
          maxValue={Math.max(0, snapshots.length - 1)}
          value={index}
          onChange={(v) => jump(v)}
          width="100%"
          marginTop="size-150"
        />
      </Panel>

      <Board visible={visible} side="them" cls={game?.opponent_class} />
      <Board visible={visible} side="us" cls={game?.player_class} />

      {snap?.unimplemented?.length > 0 && (
        <Text UNSAFE_style={{ fontSize: '.8rem', opacity: 0.75 }}>
          {t('replay.unimplemented', { cards: snap.unimplemented.join(', ') })}
        </Text>
      )}

      <Panel title={t('ui.game.inspector')}>
        <Inspector decision={decision} />
      </Panel>
    </Flex>
  )
}

function Inspector({ decision }) {
  const { t } = useT()
  if (!decision) return <Text>{t('inspector.none')}</Text>

  const expl = decision.explanation || {}
  return (
    <Flex direction="column" gap="size-150">
      <Flex direction="row" gap="size-150" alignItems="center" wrap>
        <Badge variant={decision.side === 'us' ? 'info' : 'neutral'}>
          {decision.side === 'us' ? t('ui.game.side_us') : t('ui.game.side_them')}
        </Badge>
        <Text UNSAFE_style={{ opacity: 0.75 }}>{t(`kind.${decision.kind || 'unknown'}`)}</Text>
        <Text UNSAFE_style={{ fontWeight: 600 }}>
          {t('inspector.chosen')} {decision.chosen?.card || t('replay.card_unknown')}
        </Text>
        {decision.label && (
          <Badge variant={decision.label === 'missed_lethal' ? 'negative' : 'positive'}>
            {t(`label.${decision.label}`)}
          </Badge>
        )}
      </Flex>

      {expl.what && <Text>{msgText(expl.what, t)}</Text>}

      {expl.why_bad?.length > 0 && (
        <View>
          <Text UNSAFE_style={{ fontWeight: 600 }}>{t('review.why_bad')}</Text>
          <ul style={{ margin: '.25rem 0', paddingInlineStart: '1.1em' }}>
            {msgList(expl.why_bad, t).map((x, i) => (
              <li key={i}>
                <Text>{x}</Text>
              </li>
            ))}
          </ul>
        </View>
      )}

      {expl.better?.length > 0 && (
        <View>
          <Text UNSAFE_style={{ fontWeight: 600 }}>{t('inspector.alt_better')}</Text>
          <ul style={{ margin: '.25rem 0', paddingInlineStart: '1.1em' }}>
            {expl.better.map((x, i) => (
              <li key={i}>
                <Text>{typeof x === 'string' ? x : x.line || JSON.stringify(x)}</Text>
              </li>
            ))}
          </ul>
        </View>
      )}

      {expl.strategic?.length > 0 && (
        <View>
          <Text UNSAFE_style={{ fontWeight: 600 }}>{t('inspector.strategic')}</Text>
          <ul style={{ margin: '.25rem 0', paddingInlineStart: '1.1em' }}>
            {expl.strategic.map((x, i) => (
              <li key={i}>
                <Text>{msgText(x?.text ?? x, t)}</Text>
              </li>
            ))}
          </ul>
        </View>
      )}

      {!decision.lethal_checked && decision.side === 'us' && (
        <Text UNSAFE_style={{ fontSize: '.8rem', opacity: 0.75 }}>{t('review.lethal_not_checked')}</Text>
      )}
      {decision.label_reason && (
        <Text UNSAFE_style={{ fontSize: '.8rem', opacity: 0.75 }}>
          {t('review.no_label')} {msgText(decision.label_reason, t)}
        </Text>
      )}

      <Divider size="S" />
      <Caveats items={msgList(expl.caveats, t)} title={t('inspector.why_wrong')} />
    </Flex>
  )
}
