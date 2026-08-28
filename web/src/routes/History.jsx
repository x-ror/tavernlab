import { useEffect, useState } from 'react'
import { Flex, Text } from '@adobe/react-spectrum'
import * as api from '../api'
import { useT } from '../i18n'
import ClassCrest from '../components/ClassCrest'
import { CopyButton, ErrorNote, Loading, Panel, RateBar, Stat } from '../components/ui'
import { classColor } from '../classes'
import { pct, plural } from '../format'

/* The games you actually played.
 *
 * Everything else in this app is a simulation: a number the engine
 * produced by playing the same position a thousand times. This tab is
 * the one place that shows what happened, and the two must not be
 * confused — so nothing here is estimated, smoothed or predicted. A
 * column the log could not read is blank.
 *
 * The record is a SQLite file written by `tavernsim watch`, and the path
 * to it is on the page on purpose. It is yours: copyable, queryable, and
 * not a cache this program may decide to rebuild.
 */

/** A win rate, or nothing, on the same floor the server applies.
 *
 *  The server sends `rate: null` below the floor rather than a small
 *  number, so this cannot render four games as a hundred per cent even
 *  by accident. */
function Rate({ value }) {
  const { t } = useT()
  if (value === null || value === undefined) {
    return <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>{t('ui.history.too_few')}</Text>
  }
  return <Text>{pct(value)}</Text>
}

function when(unix, lang) {
  if (!unix) return '—'
  return new Date(unix * 1000).toLocaleString(lang === 'uk' ? 'uk-UA' : 'en-GB', {
    day: '2-digit',
    month: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** One grouping, as a column of bars. Empty groups are not rendered:
 *  an empty panel titled "by opponent class" says less than no panel. */
function Group({ title, rows, hint }) {
  const { t, lang } = useT()
  if (!rows?.length) return null
  return (
    <Panel title={title}>
      {hint && (
        <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '.85rem' }}>{hint}</Text>
      )}
      <div style={{ marginTop: hint ? 12 : 0 }}>
        {rows.map((r) => (
          <Flex key={r.key} direction="row" alignItems="center" gap="size-150">
            <div style={{ flex: 1, minWidth: 0 }}>
              {/* Below the floor there is no bar either: a bar drawn at
                * 100% from four games is the same lie as the number. */}
              {r.rate === null || r.rate === undefined ? (
                <Flex direction="row" alignItems="center" gap="size-150" marginBottom="size-100">
                  <ClassCrest cls={r.key} size={16} />
                  <Text UNSAFE_style={{ flex: 1, fontSize: '.88rem' }}>{t(`class.${r.key}`)}</Text>
                  <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '.82rem' }}>
                    {plural(t, 'ui.history.n_games', r.games, lang)}
                  </Text>
                </Flex>
              ) : (
                <RateBar name={t(`class.${r.key}`)} value={r.rate} cls={r.key} />
              )}
            </div>
            <Text
              UNSAFE_style={{
                flex: '0 0 auto',
                color: 'var(--tl-muted)',
                fontSize: '.8rem',
                minWidth: '4.5rem',
                textAlign: 'right',
              }}
            >
              {r.wins}/{r.games}
            </Text>
          </Flex>
        ))}
      </div>
    </Panel>
  )
}

/** The bars want a class name to colour themselves by; a deck name is
 *  not one, so that group is drawn plainly. */
function DeckGroup({ title, rows }) {
  const { t, lang } = useT()
  if (!rows?.length) return null
  return (
    <Panel title={title}>
      <table className="tl-table" style={{ width: '100%' }}>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key}>
              <td>{r.key}</td>
              <td style={{ textAlign: 'right', color: 'var(--tl-muted)' }}>
                {plural(t, 'ui.history.n_games', r.games, lang)}
              </td>
              <td style={{ textAlign: 'right', minWidth: '5rem' }}>
                <Rate value={r.rate} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </Panel>
  )
}

export default function History() {
  const { t, lang } = useT()
  const [data, setData] = useState(undefined) // undefined = loading
  const [error, setError] = useState(null)

  useEffect(() => {
    let live = true
    let got = false
    const load = () =>
      api
        .get('/api/history')
        .then((d) => {
          if (!live) return
          got = true
          setError(null)
          setData(d)
        })
        .catch((e) => {
          if (!live) return
          // A later poll failing must not blank a record already on screen:
          // the watcher writes this file while we read it, and a blip is
          // not "your games are gone".
          if (!got) setError(e.message)
        })
    load()
    const id = setInterval(load, 2500)
    return () => {
      live = false
      clearInterval(id)
    }
  }, [])

  if (error) return <ErrorNote error={error} />
  if (data === undefined) return <Loading />
  if (data.error) return <ErrorNote error={data.error} />

  const resolved = data.resolved || 0
  const overall = resolved >= 5 ? data.wins / resolved : null

  return (
    <Flex direction="column" gap="size-300">
      <Panel title={t('ui.history.title')}>
        <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>{t('ui.history.intro')}</Text>
      </Panel>

      {!data.games ? (
        <Panel>
          <Text>{t('ui.history.empty')}</Text>
          <div style={{ marginTop: 10 }}>
            <code className="tl-code">tavernsim watch --me &lt;battletag&gt;</code>
          </div>
        </Panel>
      ) : (
        <>
          <Flex direction="row" gap="size-200" wrap>
            <Stat label={t('ui.history.games')} value={data.games} />
            <Stat
              label={t('ui.history.wins')}
              value={`${data.wins} / ${resolved}`}
              hint={resolved < data.games ? t('ui.history.unresolved', { n: data.games - resolved }) : null}
            />
            <Stat
              label={t('ui.history.winrate')}
              value={overall === null ? t('ui.history.too_few') : pct(overall)}
              tone={overall === null ? undefined : overall >= 0.5 ? 'pos' : 'neg'}
            />
          </Flex>

          <Group
            title={t('ui.history.by_opponent')}
            rows={data.by_opponent}
            hint={t('ui.history.floor')}
          />
          <Group title={t('ui.history.by_my_class')} rows={data.by_my_class} />
          <DeckGroup title={t('ui.history.by_deck')} rows={data.by_opponent_deck} />

          <Panel title={t('ui.history.recent')}>
            <div style={{ overflowX: 'auto' }}>
              <table className="tl-table" style={{ width: '100%', minWidth: 620 }}>
                <thead>
                  <tr>
                    <th>{t('ui.history.col_when')}</th>
                    <th>{t('ui.history.col_you')}</th>
                    <th>{t('ui.history.col_them')}</th>
                    <th>{t('ui.history.col_result')}</th>
                    <th style={{ textAlign: 'right' }}>{t('ui.history.col_turns')}</th>
                    <th>{t('ui.history.col_read')}</th>
                  </tr>
                </thead>
                <tbody>
                  {(data.rows || []).slice(0, 100).map((g, i) => (
                    <tr key={`${g.played_at}-${i}`}>
                      <td style={{ whiteSpace: 'nowrap' }}>{when(g.played_at, lang)}</td>
                      <td>
                        <Flex direction="row" alignItems="center" gap="size-100">
                          <ClassCrest cls={g.my_class} size={15} />
                          <Text UNSAFE_style={{ color: classColor(g.my_class) }}>
                            {t(`class.${g.my_class}`)}
                          </Text>
                          {/* Who was on the draw decides a lot of games,
                            * and the log states it outright. */}
                          {g.coin && (
                            <Text
                              UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '.72rem' }}
                              title={t('ui.history.coin')}
                            >
                              ◉
                            </Text>
                          )}
                        </Flex>
                      </td>
                      <td>
                        <Flex direction="row" alignItems="center" gap="size-100">
                          <ClassCrest cls={g.opponent_class} size={15} />
                          <Text UNSAFE_style={{ color: classColor(g.opponent_class) }}>
                            {t(`class.${g.opponent_class}`)}
                          </Text>
                        </Flex>
                      </td>
                      <td>
                        {g.won === null || g.won === undefined ? (
                          <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>
                            {t('ui.history.unknown')}
                          </Text>
                        ) : (
                          <Text
                            UNSAFE_style={{
                              color: g.won ? 'var(--tl-pos)' : 'var(--tl-neg)',
                            }}
                          >
                            {g.won ? t('ui.history.won') : t('ui.history.lost')}
                          </Text>
                        )}
                      </td>
                      <td style={{ textAlign: 'right' }}>{g.turns || '—'}</td>
                      <td>
                        {g.opponent_deck ? (
                          <Text>
                            {g.opponent_deck}{' '}
                            <span style={{ color: 'var(--tl-muted)', fontSize: '.8rem' }}>
                              {t('ui.history.hits', {
                                hits: g.opponent_hits,
                                seen: g.opponent_seen,
                              })}
                            </span>
                          </Text>
                        ) : (
                          <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>
                            {t('ui.history.no_read')}
                          </Text>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Panel>
        </>
      )}

      <Panel title={t('ui.history.file')}>
        <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>{t('ui.history.file_note')}</Text>
        <Flex direction="row" alignItems="center" gap="size-150" marginTop="size-150" wrap>
          <code className="tl-code" style={{ wordBreak: 'break-all' }}>
            {data.path}
          </code>
          <CopyButton text={data.path}>{t('ui.history.copy_path')}</CopyButton>
        </Flex>
      </Panel>
    </Flex>
  )
}
