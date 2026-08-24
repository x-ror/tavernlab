import { useEffect, useMemo, useState } from 'react'
import {
  ActionButton,
  Button,
  Content,
  DialogTrigger,
  Flex,
  Heading,
  IllustratedMessage,
  Text,
  View,
} from '@adobe/react-spectrum'
import { go, useApp } from '../store'
import { useT } from '../i18n'
import * as api from '../api'
import ImportDialog from '../components/ImportDialog'
import { Loading, Panel, RateBar, ResultPill, Stat } from '../components/ui'
import HeroPortrait from '../components/HeroPortrait'
import { classColor, classFocal, classWash, heroArt } from '../classes'
import { useDeckClasses } from '../meta'
import { fmtDate, pct } from '../format'

// How many finished reviews the home screen reads to count missed
// lethals. Each is one local SQLite read; deeper is slower for no gain.
const SCAN = 12

export default function CoachHome() {
  const { t, lang } = useT()
  const { gamesList, deckCode, deckInfo, analysis, hasTelemetry } = useApp()
  const [missed, setMissed] = useState(null)
  const deckClasses = useDeckClasses(analysis?.format)

  // Whose portrait heads the page: the class you actually play, taken
  // from the games on file, falling back to the deck you have loaded.
  const mainClass = useMemo(() => {
    const tally = {}
    for (const g of gamesList || []) {
      if (g.player_class) tally[g.player_class] = (tally[g.player_class] || 0) + 1
    }
    const top = Object.entries(tally).sort((a, b) => b[1] - a[1])[0]
    return top ? top[0] : deckInfo?.cls || null
  }, [gamesList, deckInfo])

  const ready = useMemo(
    () => (gamesList || []).filter((g) => g.review_status === 'ready' || g.review_status === 'partial').slice(0, SCAN),
    [gamesList],
  )
  const pending = useMemo(
    () => (gamesList || []).filter((g) => g.review_status === 'pending').length,
    [gamesList],
  )

  useEffect(() => {
    let live = true
    if (!ready.length) {
      setMissed({ count: 0, games: [] })
      return
    }
    Promise.all(
      ready.map((g) =>
        api.games
          .review(g.id)
          .then((r) => ({ g, moments: (r.key_moments || []).filter((m) => m.label === 'missed_lethal') }))
          .catch(() => ({ g, moments: [] })),
      ),
    ).then((rows) => {
      if (!live) return
      const hits = rows.filter((r) => r.moments.length)
      setMissed({ count: hits.reduce((n, r) => n + r.moments.length, 0), games: hits })
    })
    return () => {
      live = false
    }
  }, [ready.map((g) => g.id).join(',')])

  if (gamesList === null) return <Loading />

  const decided = (gamesList || []).filter((g) => g.result === 'win' || g.result === 'loss')
  const wins = decided.filter((g) => g.result === 'win').length
  const winrate = decided.length ? wins / decided.length : null

  const tasks = buildTasks({ t, gamesList, pending, missed, deckCode, deckInfo, analysis, hasTelemetry })

  if (!gamesList.length && !deckCode) return <FirstRun />

  return (
    <Flex direction="column" gap="size-300">
      <Banner cls={mainClass} t={t} winrate={winrate} games={gamesList.length} />

      <Flex direction="row" gap="size-200" wrap>
        <Stat
          label={t('ui.coach.tile_games')}
          value={gamesList.length}
          accent={mainClass ? classColor(mainClass) : null}
        />
        <Stat
          label={t('ui.coach.tile_wr')}
          value={pct(winrate)}
          hint={decided.length ? t('ui.coach.tile_wr_help', { n: decided.length }) : null}
          tone={winrate === null ? null : winrate >= 0.5 ? 'pos' : 'neg'}
        />
        <Stat
          label={t('ui.coach.tile_missed')}
          value={missed ? missed.count : '…'}
          hint={t('ui.coach.tile_missed_help', { n: ready.length })}
          tone={missed && missed.count > 0 ? 'neg' : null}
        />
        <Stat
          label={t('ui.coach.tile_deck')}
          accent={analysis ? classColor(analysis.cls) : null}
          value={analysis ? pct(analysis.avg, 1) : '—'}
          hint={analysis ? t('ui.deck.ok', { cls: t(`class.${analysis.cls}`), n: '' }).replace(/,\s*$/, '') : null}
          tone={analysis ? (analysis.avg >= 0.5 ? 'pos' : 'neg') : null}
        />
      </Flex>

      <Flex direction={{ base: 'column', L: 'row' }} gap="size-300" alignItems="start">
        <View flex="1 1 60%" minWidth="size-4600" width="100%">
          <Panel title={t('ui.coach.title')}>
            {tasks.length ? (
              <Flex direction="column" gap="size-200">
                {tasks.map((task, i) => (
                  <Task key={i} task={task} t={t} />
                ))}
              </Flex>
            ) : (
              <Text>{t('ui.coach.no_tasks')}</Text>
            )}
          </Panel>
        </View>

        <View flex="1 1 40%" minWidth="size-3600" width="100%">
          <Panel
            title={t('ui.coach.recent')}
            action={<ActionButton onPress={() => go('games')}>{t('ui.coach.open')}</ActionButton>}
          >
            {gamesList.length === 0 ? (
              <Text>{t('games.empty')}</Text>
            ) : (
              <Flex direction="column">
                {gamesList.slice(0, 6).map((g, i) => (
                  <div key={g.id}>
                    {i > 0 && <hr className="tl-rule" />}
                    <button type="button" className="tl-row" onClick={() => go(`games/${g.id}`)}>
                      <Flex direction="row" alignItems="center" gap="size-150">
                        <HeroPortrait cls={g.opponent_class} size={34} flip title={g.opponent_class} />
                        <Flex direction="column" flex="1 1 auto" gap="size-25">
                          <span style={{ fontSize: '.88rem' }}>
                            <span style={{ color: 'var(--tl-faint)' }}>{t('games.vs')} </span>
                            <span style={{ color: classColor(g.opponent_class), fontWeight: 600 }}>
                              {t(`class.${g.opponent_class || 'unknown'}`)}
                            </span>
                          </span>
                          <span style={{ fontSize: '.74rem', color: 'var(--tl-muted)' }}>
                            {fmtDate(g.started_at, lang)} · {t('review.turns_n', { n: g.turns ?? '—' })}
                          </span>
                        </Flex>
                        <ResultPill result={g.result} label={t(`result.${g.result || 'unknown'}`)} />
                      </Flex>
                    </button>
                  </div>
                ))}
              </Flex>
            )}
          </Panel>

          {analysis && (
            <View marginTop="size-300">
              <Panel
                title={t('ui.deck.tab_rating')}
                action={<ActionButton onPress={() => go('deck/rating')}>{t('ui.coach.open')}</ActionButton>}
              >
                {Object.entries(analysis.rates || {})
                  .sort((a, b) => a[1] - b[1])
                  .slice(0, 5)
                  .map(([name, v]) => (
                    <RateBar key={name} name={name} value={v} cls={deckClasses[name]} />
                  ))}
              </Panel>
            </View>
          )}
        </View>
      </Flex>
    </Flex>
  )
}

/* The page opens with the hero you actually play. It is not decoration:
 * it says "this is your record", and the class colour it establishes is
 * the same one every bar, crest and matchup line uses below. */
function Banner({ cls, t, winrate, games }) {
  const art = heroArt(cls)
  return (
    <div
      className="tl-banner"
      style={{ background: `linear-gradient(100deg, ${classWash(cls, 0.24)} 0%, var(--tl-surface) 58%, var(--tl-bg-2) 100%)` }}
    >
      {/* No medallion in front of it: the art is sharp and framed by
          `classFocal`, so a second, smaller copy of the same face would
          only compete with it. */}
      {art && (
        <img
          className="tl-banner-art"
          src={art}
          alt=""
          style={{ objectPosition: classFocal(cls) }}
        />
      )}
      <Flex direction="column" gap="size-100" UNSAFE_style={{ position: 'relative' }}>
        <span className="tl-eyebrow">{t('ui.coach.your_class')}</span>
        <span
          className="tl-banner-name"
          style={{ color: cls ? classColor(cls) : 'var(--tl-text)' }}
        >
          {cls ? t(`class.${cls}`) : t('ui.coach.no_class')}
        </span>
        <span className="tl-banner-stats">
          {t('ui.coach.tile_games')}: <b>{games}</b>
          {winrate !== null ? (
            <>
              {' · '}
              {t('ui.coach.tile_wr')} <b>{pct(winrate)}</b>
            </>
          ) : null}
        </span>
      </Flex>
    </div>
  )
}


function Task({ task, t }) {
  return (
    <View
      borderStartWidth="thick"
      borderStartColor={task.tone === 'neg' ? 'negative' : task.tone === 'pos' ? 'positive' : 'informative'}
      paddingStart="size-200"
    >
      <Flex direction="row" justifyContent="space-between" alignItems="center" gap="size-200" wrap>
        <Flex direction="column" flex="1 1 auto" minWidth="size-3000">
          <Text UNSAFE_style={{ fontWeight: 600 }}>{task.title}</Text>
          <Text UNSAFE_style={{ fontSize: '.85rem', opacity: 0.8 }}>{task.body}</Text>
        </Flex>
        {task.action}
      </Flex>
    </View>
  )
}

/* The home screen has one job: name the next concrete thing to do.
 * Every task below is derived from a number the app actually has —
 * none of them is a generic "play better" nudge. */
function buildTasks({ t, gamesList, pending, missed, deckCode, deckInfo, analysis, hasTelemetry }) {
  const tasks = []

  if (!gamesList.length) {
    tasks.push({
      title: t('ui.coach.task_import'),
      body: t('ui.coach.task_import_body'),
      tone: 'neg',
      action: (
        <DialogTrigger>
          <Button variant="accent">{t('ui.games.import')}</Button>
          {(close) => <ImportDialog close={close} />}
        </DialogTrigger>
      ),
    })
  }

  if (missed && missed.count > 0) {
    const first = missed.games[0]
    tasks.push({
      title: t('ui.coach.task_lethal', { n: missed.count }),
      body: t('ui.coach.task_lethal_body'),
      tone: 'neg',
      action: (
        <Button variant="secondary" onPress={() => go(`games/${first.g.id}/review`)}>
          {t('ui.coach.open')}
        </Button>
      ),
    })
  }

  if (!deckCode || !deckInfo?.ok || !hasTelemetry) {
    tasks.push({
      title: t('ui.coach.task_deck'),
      body: t('ui.coach.task_deck_body'),
      tone: 'neutral',
      action: (
        <Button variant="secondary" onPress={() => go('deck/rating')}>
          {t('ui.coach.open')}
        </Button>
      ),
    })
  }

  const weak = analysis?.coach?.weak?.[0]
  if (weak) {
    tasks.push({
      title: t('ui.coach.task_weak', { n: weak[0], v: Math.round(weak[1] * 100) }),
      body: t('ui.coach.task_weak_body'),
      tone: weak[1] < 0.45 ? 'neg' : 'neutral',
      action: (
        <Button variant="secondary" onPress={() => go('deck/coach')}>
          {t('ui.coach.open')}
        </Button>
      ),
    })
  }

  if (pending > 0) {
    tasks.push({
      title: t('ui.coach.task_review', { n: pending }),
      body: t('ui.coach.task_review_body'),
      tone: 'neutral',
      action: (
        <Button variant="secondary" onPress={() => go('games')}>
          {t('ui.coach.open')}
        </Button>
      ),
    })
  }

  return tasks
}

function FirstRun() {
  const { t } = useT()
  return (
    <View backgroundColor="gray-100" borderRadius="medium" borderWidth="thin" borderColor="gray-300" padding="size-600">
      <IllustratedMessage>
        <Heading>{t('ui.coach.empty_title')}</Heading>
        <Content>
          <Flex direction="column" gap="size-250" alignItems="center">
            <Text>{t('ui.coach.empty_body')}</Text>
            <Flex direction="row" gap="size-150">
              <DialogTrigger>
                <Button variant="accent">{t('ui.games.import')}</Button>
                {(close) => <ImportDialog close={close} />}
              </DialogTrigger>
              <Button variant="secondary" onPress={() => go('deck/rating')}>
                {t('ui.deck.set')}
              </Button>
            </Flex>
          </Flex>
        </Content>
      </IllustratedMessage>
    </View>
  )
}
