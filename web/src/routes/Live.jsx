import { useEffect, useRef, useState } from 'react'
import { Button, Flex, StatusLight, Switch, Text, TextField } from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { useT } from '../i18n'
import { ErrorNote, Panel } from '../components/ui'

/* Advice while you play.
 *
 * The watcher reads the game's own log — the file the client writes when
 * `log.config` asks it to — and says what to keep, what the opponent is
 * playing and what to do this turn. It used to be a second command in a
 * second terminal; it runs inside this server now, so the deck pasted on
 * the Deck tab is the deck it advises on and the games it sees land in
 * History without anything else being started.
 *
 * The position it read is shown under the advice on purpose. A log is a
 * partial view — the opponent's hand is face down — and showing what was
 * rebuilt is what makes a wrong read visible instead of silent.
 */

/* Once a second. The watcher polls the log at its own cadence and keeps
 * the answer; this only asks what the answer currently is, so a page left
 * open costs a struct read per second and never falls behind. */
const POLL_MS = 1000

export default function Live() {
  const { t } = useT()
  const { settings, saveSettings } = useApp()
  const [live, setLive] = useState(null)
  const [error, setError] = useState(null)
  const [busy, setBusy] = useState(false)
  const [dir, setDir] = useState('')
  const dirTouched = useRef(false)

  useEffect(() => {
    let on = true
    const tick = () =>
      api
        .get('/api/live')
        .then((d) => {
          if (!on) return
          setLive(d)
          // The server's guess fills the field until the user edits it;
          // after that the field is theirs and is left alone.
          if (!dirTouched.current) setDir(d.logs_dir || '')
        })
        .catch(() => {})
    tick()
    const id = setInterval(tick, POLL_MS)
    return () => {
      on = false
      clearInterval(id)
    }
  }, [])

  const act = async (action) => {
    setBusy(true)
    setError(null)
    try {
      if (action === 'start' && dirTouched.current) await saveSettings({ logs_dir: dir.trim() })
      setLive(await api.post('/api/live', { action }))
      dirTouched.current = false
    } catch (e) {
      setError(e.message)
    } finally {
      setBusy(false)
    }
  }

  const running = !!live?.running
  const deck = settings?.deckstring || ''

  return (
    <Flex direction="column" gap="size-300">
      <Panel
        title={t('ui.live.title')}
        action={
          <Flex direction="row" gap="size-150" alignItems="center">
            <StatusLight variant={running ? 'positive' : 'neutral'}>
              {running ? t('ui.live.on') : t('ui.live.off')}
            </StatusLight>
            <Button
              variant={running ? 'secondary' : 'accent'}
              isDisabled={busy}
              onPress={() => act(running ? 'stop' : 'start')}
            >
              {running ? t('ui.live.stop') : t('ui.live.start')}
            </Button>
          </Flex>
        }
      >
        <Text UNSAFE_style={{ fontSize: '.9rem', opacity: 0.85 }}>{t('ui.live.intro')}</Text>
        <Flex direction="column" gap="size-200" marginTop="size-250" maxWidth="size-6000">
          <TextField
            label={t('ui.live.logs_dir')}
            description={t('ui.live.logs_help')}
            value={dir}
            onChange={(v) => {
              dirTouched.current = true
              setDir(v)
            }}
            width="100%"
          />
          <Switch
            isSelected={settings?.live_auto === 'on'}
            onChange={(v) => saveSettings({ live_auto: v ? 'on' : '' })}
          >
            {t('ui.live.auto')}
          </Switch>
        </Flex>
        {error && <ErrorNote error={error} />}
        {!deck && (
          <Text UNSAFE_style={{ display: 'block', marginTop: 12, color: 'var(--tl-warn)' }}>
            {t('ui.live.no_deck')}
          </Text>
        )}
        {live?.note && (
          <Text UNSAFE_style={{ display: 'block', marginTop: 12, color: 'var(--tl-muted)' }}>
            {live.note}
          </Text>
        )}
        {running && live?.watching && (
          <Text
            UNSAFE_style={{
              display: 'block',
              marginTop: 8,
              fontSize: '.8rem',
              color: 'var(--tl-muted)',
              wordBreak: 'break-all',
            }}
          >
            {live.watching}
          </Text>
        )}
      </Panel>

      <Advice live={live} />
    </Flex>
  )
}

/* One section per heading, in the order the watcher built them: what to
 * do first, then what the opponent looks like, then the position it read.
 * A heading with no lines under it is never sent, so nothing here renders
 * an empty block that reads as "no advice". */
function Advice({ live }) {
  const { t } = useT()
  if (!live?.title) return null
  return (
    <Panel title={live.title}>
      {live.sections?.length ? (
        live.sections.map((s) => (
          <div key={s.heading} style={{ marginBottom: 18 }}>
            <div
              style={{
                fontSize: '.75rem',
                letterSpacing: '.08em',
                color: 'var(--tl-muted)',
                marginBottom: 6,
              }}
            >
              {s.heading}
            </div>
            <ul style={{ margin: 0, paddingLeft: 20, lineHeight: 1.7 }}>
              {s.lines.map((line, i) => (
                <li key={i}>{line}</li>
              ))}
            </ul>
          </div>
        ))
      ) : (
        <Text UNSAFE_style={{ color: 'var(--tl-muted)' }}>{t('ui.live.nothing_yet')}</Text>
      )}
    </Panel>
  )
}
