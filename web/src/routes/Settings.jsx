import { useEffect, useState } from 'react'
import {
  Button,
  Flex,
  Item,
  Picker,
  Radio,
  RadioGroup,
  StatusLight,
  Switch,
  Text,
  TextField,
  View,
} from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { useT } from '../i18n'
import { Caveats, Panel } from '../components/ui'
import { pct } from '../format'

export default function Settings() {
  const { t } = useT()
  const { settings, saveSettings } = useApp()
  const [draft, setDraft] = useState(settings || {})
  const [saved, setSaved] = useState(false)

  useEffect(() => setDraft(settings || {}), [settings])

  const set = (k) => (v) => {
    setDraft((d) => ({ ...d, [k]: v }))
    setSaved(false)
  }

  return (
    <Flex direction="column" gap="size-300">
      <Panel title={t('ui.nav.settings')}>
        <Flex direction="column" gap="size-250" maxWidth="size-6000">
          <TextField
            label={t('settings.logs_dir')}
            placeholder={t('settings.logs_dir_ph')}
            value={draft.logs_dir || ''}
            onChange={set('logs_dir')}
            width="100%"
          />
          <TextField
            label={t('settings.player_name')}
            placeholder={t('settings.player_name_ph')}
            description={t('settings.player_name_hint')}
            value={draft.player_name || ''}
            onChange={set('player_name')}
            width="100%"
          />
          <TextField
            label={t('settings.deckstring')}
            placeholder={t('deck.ph')}
            value={draft.deckstring || ''}
            onChange={set('deckstring')}
            width="100%"
          />
          <Picker
            label={t('settings.language')}
            selectedKey={draft.language || 'auto'}
            onSelectionChange={(k) => set('language')(k === 'auto' ? '' : String(k))}
            width="size-3000"
          >
            <Item key="auto">{t('settings.lang_auto')}</Item>
            <Item key="uk">{t('settings.lang_uk')}</Item>
            <Item key="en">{t('settings.lang_en')}</Item>
          </Picker>

          <Flex direction="row" gap="size-200" alignItems="center">
            <Button
              variant="accent"
              onPress={async () => {
                await saveSettings(draft)
                setSaved(true)
              }}
            >
              {t('settings.save')}
            </Button>
            {saved && <StatusLight variant="positive">{t('settings.saved')}</StatusLight>}
          </Flex>
        </Flex>
      </Panel>

      <Panel title={t('settings.live_title')}>
        <Flex direction="column" gap="size-200" maxWidth="size-6000">
          <Switch
            isSelected={draft.live_eval === '1'}
            onChange={(on) => set('live_eval')(on ? '1' : '0')}
          >
            {t('settings.live_eval')}
          </Switch>
          <RadioGroup
            label={t('settings.live_default_off')}
            value={draft.live_lethal_mode || 'line'}
            onChange={set('live_lethal_mode')}
            isDisabled={draft.live_eval !== '1'}
          >
            <Radio value="line">{t('settings.live_mode_line')}</Radio>
            <Radio value="hint">{t('settings.live_mode_hint')}</Radio>
          </RadioGroup>
          <Caveats items={[t('settings.live_warn'), t('import.readonly')]} />
        </Flex>
      </Panel>

      <Diagnostics />
    </Flex>
  )
}

/* `/api/metrics` is deliberately boring: local counters, no phone-home.
 * `pct_search_ok` reads 0 in this build and is shown anyway, because the
 * design measures that number rather than assuming it. */
function Diagnostics() {
  const { t } = useT()
  const [m, setM] = useState(null)

  useEffect(() => {
    api.metrics().then(setM).catch(() => setM(null))
  }, [])

  if (!m) return null

  const rows = [
    [t('ui.coach.tile_games'), m.games],
    ['reviews_run', m.reviews_run],
    ['decisions', m.decisions],
    ['pct_lethal_ok', m.pct_lethal_ok === null ? '—' : pct(m.pct_lethal_ok, 1)],
    ['pct_search_ok', m.pct_search_ok === null ? '—' : pct(m.pct_search_ok, 1)],
    ['mean_review_ms', m.mean_review_ms],
    ['errors', m.errors],
    ['log', m.log_path],
  ]

  return (
    <Panel title={t('ui.settings.diag')}>
      <Text UNSAFE_style={{ fontSize: '.85rem', opacity: 0.8 }}>{t('ui.settings.diag_help')}</Text>
      <View marginTop="size-200">
        {rows.map(([k, v]) => (
          <Flex key={k} direction="row" justifyContent="space-between" gap="size-200" marginBottom="size-75">
            <Text UNSAFE_style={{ opacity: 0.75 }}>{k}</Text>
            <Text UNSAFE_style={{ fontVariantNumeric: 'tabular-nums', wordBreak: 'break-all' }}>
              {v ?? '—'}
            </Text>
          </Flex>
        ))}
      </View>
    </Panel>
  )
}
