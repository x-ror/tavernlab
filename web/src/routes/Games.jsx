import { useEffect, useState } from 'react'
import {
  ActionButton,
  Button,
  Content,
  DialogTrigger,
  Flex,
  Heading,
  IllustratedMessage,
  Item,
  Picker,
  Text,
} from '@adobe/react-spectrum'
import { go, useApp } from '../store'
import { useT } from '../i18n'
import ImportDialog from '../components/ImportDialog'
import HeroPortrait from '../components/HeroPortrait'
import { ErrorNote, Loading, Panel, ResultPill } from '../components/ui'
import { CLASS_KEYS as CLASSES, classColor } from '../classes'
import { archetype, fmtDate, reviewVariant } from '../format'

const RESULTS = ['win', 'loss', 'tie', 'unknown']

const STATUS_DOT = {
  positive: 'var(--tl-pos)',
  info: 'var(--tl-accent)',
  yellow: 'var(--tl-warn)',
  negative: 'var(--tl-neg)',
  neutral: 'var(--tl-faint)',
}

export default function Games() {
  const { t } = useT()
  const { gamesList, gamesError, refreshGames } = useApp()
  const [cls, setCls] = useState('any')
  const [result, setResult] = useState('any')

  useEffect(() => {
    refreshGames({ cls: cls === 'any' ? null : cls, result: result === 'any' ? null : result })
  }, [cls, result, refreshGames])

  if (gamesError) return <ErrorNote error={gamesError} />
  if (gamesList === null) return <Loading />

  const refresh = () =>
    refreshGames({ cls: cls === 'any' ? null : cls, result: result === 'any' ? null : result })

  return (
    <Flex direction="column" gap="size-300">
      <Flex direction="row" gap="size-200" alignItems="end" wrap>
        <Picker
          label={t('games.filter_class')}
          items={[
            { id: 'any', name: t('games.any') },
            ...CLASSES.map((c) => ({ id: c, name: t(`class.${c}`) })),
          ]}
          selectedKey={cls}
          onSelectionChange={(k) => setCls(String(k))}
        >
          {(item) => <Item>{item.name}</Item>}
        </Picker>
        <Picker
          label={t('games.filter_result')}
          items={[
            { id: 'any', name: t('games.any') },
            ...RESULTS.map((r) => ({ id: r, name: t(`result.${r}`) })),
          ]}
          selectedKey={result}
          onSelectionChange={(k) => setResult(String(k))}
        >
          {(item) => <Item>{item.name}</Item>}
        </Picker>
        <span style={{ flex: '1 1 auto' }} />
        <Text UNSAFE_style={{ color: 'var(--tl-muted)', paddingBottom: 6 }}>
          {t('ui.games.count', { n: gamesList.length })}
        </Text>
        <ActionButton onPress={refresh}>{t('games.refresh')}</ActionButton>
        <DialogTrigger>
          <Button variant="accent">{t('ui.games.import')}</Button>
          {(close) => <ImportDialog close={close} />}
        </DialogTrigger>
      </Flex>

      {gamesList.length === 0 ? (
        <EmptyGames />
      ) : (
        <Panel style={{ padding: '10px 14px' }}>
          {gamesList.map((g, i) => (
            <GameRow key={g.id} game={g} first={i === 0} />
          ))}
        </Panel>
      )}
    </Flex>
  )
}

/* A row is a button, not a table cell: the whole strip is the target,
 * and the two portraits carry the matchup faster than the words do. */
function GameRow({ game: g, first }) {
  const { t, lang } = useT()
  const arch = archetype(g, t)
  const dot = STATUS_DOT[reviewVariant(g.review_status)] || STATUS_DOT.neutral

  return (
    <>
      {!first && <hr className="tl-rule" />}
      <button type="button" className="tl-row" onClick={() => go(`games/${g.id}`)}>
        <Flex direction="row" alignItems="center" gap="size-200" wrap>
          <Flex direction="row" alignItems="center" gap="size-75">
            <HeroPortrait cls={g.player_class} size={42} title={g.player_class} />
            <HeroPortrait cls={g.opponent_class} size={42} title={g.opponent_class} flip dim />
          </Flex>

          <Flex direction="column" gap="size-25" flex="1 1 14rem" minWidth="size-2400">
            <span style={{ fontSize: '.94rem', fontWeight: 600 }}>
              <span style={{ color: classColor(g.player_class) }}>
                {t(`class.${g.player_class || 'unknown'}`)}
              </span>
              <span style={{ color: 'var(--tl-faint)', margin: '0 .4em' }}>{t('games.vs')}</span>
              <span style={{ color: classColor(g.opponent_class) }}>
                {t(`class.${g.opponent_class || 'unknown'}`)}
              </span>
            </span>
            <span style={{ fontSize: '.76rem', color: 'var(--tl-muted)' }}>
              {fmtDate(g.started_at, lang)} · {g.going_first ? t('games.first') : t('games.second')}
              {arch ? ` · ${arch}` : ''}
            </span>
          </Flex>

          <span
            style={{
              fontSize: '.8rem',
              color: 'var(--tl-muted)',
              fontVariantNumeric: 'tabular-nums',
              minWidth: '4.5rem',
            }}
          >
            {t('review.turns_n', { n: g.turns ?? '—' })}
          </span>

          <ResultPill result={g.result} label={t(`result.${g.result || 'unknown'}`)} />

          <span
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 6,
              minWidth: '6.5rem',
              fontSize: '.76rem',
              color: 'var(--tl-muted)',
            }}
          >
            {g.reviewable === false ? (
              t('games.blocked')
            ) : (
              <>
                <i
                  aria-hidden="true"
                  style={{
                    width: 7,
                    height: 7,
                    borderRadius: '50%',
                    background: dot,
                    display: 'block',
                  }}
                />
                {t(`review_status.${g.review_status || 'none'}`)}
              </>
            )}
          </span>

          <span aria-hidden="true" style={{ color: 'var(--tl-faint)' }}>
            ›
          </span>
        </Flex>
      </button>
    </>
  )
}

function EmptyGames() {
  const { t } = useT()
  return (
    <Panel style={{ padding: '40px 24px' }}>
      <IllustratedMessage>
        <Heading>{t('games.empty')}</Heading>
        <Content>
          <Flex direction="column" gap="size-200" alignItems="center">
            <Text>{t('games.empty_hint')}</Text>
            <DialogTrigger>
              <Button variant="accent">{t('games.empty_cta')}</Button>
              {(close) => <ImportDialog close={close} />}
            </DialogTrigger>
          </Flex>
        </Content>
      </IllustratedMessage>
    </Panel>
  )
}
