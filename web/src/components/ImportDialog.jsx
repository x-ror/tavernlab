import { useState } from 'react'
import {
  Button,
  ButtonGroup,
  Content,
  Dialog,
  Divider,
  Flex,
  Heading,
  ProgressBar,
  Text,
  TextField,
} from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { useT } from '../i18n'
import { ErrorNote } from './ui'

const CFG_PATH = String.raw`%LOCALAPPDATA%\Blizzard\Hearthstone\log.config`

// Verbatim from the desktop UI: the client only writes the detailed log
// when this file says so, and a snippet that differs by one line is a
// support ticket.
const LOG_CONFIG = `[Power]
LogLevel=1
FilePrinting=true
ConsolePrinting=false
ScreenPrinting=false
Verbose=true
[Zone]
LogLevel=1
FilePrinting=true`

/* Import is a dialog, not a tab: it is something you do once in a while,
 * not a place you live. The log.config snippet stays copy-only — the app
 * never writes into the Hearthstone folder (README, "Legal mode"). */
export default function ImportDialog({ close }) {
  const { t } = useT()
  const { settings, saveSettings, refreshGames } = useApp()
  const [path, setPath] = useState('')
  const [logsDir, setLogsDir] = useState(settings?.logs_dir || '')
  const [busy, setBusy] = useState(null)
  const [progress, setProgress] = useState([])
  const [error, setError] = useState(null)
  const [done, setDone] = useState(null)
  const [copied, setCopied] = useState(false)

  async function run(kind) {
    setError(null)
    setDone(null)
    setProgress([])
    setBusy(kind)
    try {
      if (kind === 'last' && logsDir && logsDir !== settings?.logs_dir) {
        await saveSettings({ logs_dir: logsDir })
      }
      const result =
        kind === 'last'
          ? await api.runJob('/api/import/last_session', { logs_dir: logsDir, only_last: true }, setProgress)
          : await api.runJob('/api/import/log', { path, only_last: false }, setProgress)
      setDone(result)
      await refreshGames()
    } catch (e) {
      setError(e.message)
    } finally {
      setBusy(null)
    }
  }

  // job_import returns {games: [id, ...], path}
  const imported = Array.isArray(done?.games) ? done.games.length : null

  return (
    <Dialog width="size-6000">
      <Heading>{t('ui.games.import')}</Heading>
      <Divider />
      <Content>
        <Text>{t('import.intro')}</Text>

        <Flex direction="column" gap="size-150" marginTop="size-250">
          <TextField
            label={t('settings.logs_dir')}
            placeholder={t('settings.logs_dir_ph')}
            value={logsDir}
            onChange={setLogsDir}
            width="100%"
          />
          <Button variant="accent" isDisabled={!!busy || !logsDir.trim()} onPress={() => run('last')}>
            {t('import.btn_last')}
          </Button>
        </Flex>

        <Divider size="S" marginY="size-250" />

        <Flex direction="column" gap="size-150">
          <TextField
            label={t('import.path_label')}
            placeholder={t('import.path_ph')}
            value={path}
            onChange={setPath}
            width="100%"
          />
          <Button variant="secondary" isDisabled={!!busy || !path.trim()} onPress={() => run('log')}>
            {t('import.btn_log')}
          </Button>
        </Flex>

        {busy && (
          <Flex direction="column" gap="size-100" marginTop="size-250">
            <ProgressBar isIndeterminate label={t('common.loading')} width="100%" />
            {progress.length > 0 && (
              <div className="tl-mono" style={{ maxHeight: 120, overflow: 'auto' }}>
                {progress.slice(-8).join('\n')}
              </div>
            )}
          </Flex>
        )}

        <ErrorNote error={error} />

        {done && (
          <Text UNSAFE_style={{ display: 'block', marginTop: '1rem', color: 'var(--tl-pos)' }}>
            {t('import.done', { n: imported ?? '—' })}
          </Text>
        )}

        <Divider size="S" marginY="size-250" />

        <Heading level={4}>{t('import.cfg_title')}</Heading>
        <Text>{t('import.cfg_help')}</Text>
        <Text UNSAFE_style={{ display: 'block', marginTop: '.5rem', fontSize: '.8rem', opacity: 0.75 }}>
          {t('import.cfg_where')}
        </Text>
        <div className="tl-mono">{CFG_PATH}</div>
        <div className="tl-mono" style={{ marginTop: '.5rem' }}>{LOG_CONFIG}</div>
        <Flex direction="row" gap="size-150" alignItems="center" marginTop="size-150">
          <Button
            variant="secondary"
            onPress={() => {
              navigator.clipboard?.writeText(LOG_CONFIG)
              setCopied(true)
            }}
          >
            {copied ? t('import.copied') : t('import.copy')}
          </Button>
          <Text UNSAFE_style={{ fontSize: '.8rem', opacity: 0.75 }}>{t('import.manual_only')}</Text>
        </Flex>
      </Content>
      <ButtonGroup>
        <Button variant="primary" onPress={close}>
          {t('ui.common.close')}
        </Button>
      </ButtonGroup>
    </Dialog>
  )
}
