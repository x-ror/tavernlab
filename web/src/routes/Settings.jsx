import { useEffect, useState } from 'react'
import { Button, Flex, Item, Picker, StatusLight, Text, TextField, View } from '@adobe/react-spectrum'
import * as api from '../api'
import { useApp } from '../store'
import { useT } from '../i18n'
import { Panel } from '../components/ui'
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

      <Diagnostics />
    </Flex>
  )
}

/* `/api/metrics` is deliberately boring: what this build of the engine
 * can do, and what it has done since it started. Nothing is sent
 * anywhere, and the coverage numbers are measured from the card table
 * rather than claimed. */
function Diagnostics() {
  const { t } = useT()
  const [m, setM] = useState(null)

  useEffect(() => {
    api
      .metrics()
      .then(setM)
      .catch(() => setM(null))
  }, [])

  if (!m) return null

  const coverage = (done, all) => `${done} / ${all} · ${pct(all ? done / all : null, 1)}`
  const field = (g) => (g ? `${g.playable} / ${g.decks}` : '—')
  const rows = [
    [t('ui.settings.cards'), m.cards],
    [t('ui.settings.impl_standard'), coverage(m.standard_implemented, m.standard_deckable)],
    [t('ui.settings.impl_wild'), coverage(m.wild_implemented, m.wild_deckable)],
    [t('ui.settings.field_standard'), field(m.gauntlet_standard)],
    [t('ui.settings.field_wild'), field(m.gauntlet_wild)],
    [t('ui.settings.games'), m.games_simulated],
    [t('ui.settings.threads'), m.threads],
    [t('ui.settings.data_home'), m.data_home],
    [t('ui.settings.root'), m.root],
  ]

  return (
    <Panel title={t('ui.settings.diag')}>
      <Text UNSAFE_style={{ fontSize: '.85rem', opacity: 0.8 }}>{t('ui.settings.diag_help')}</Text>
      <View marginTop="size-200">
        {rows.map(([k, v]) => (
          <Flex
            key={k}
            direction="row"
            justifyContent="space-between"
            gap="size-200"
            marginBottom="size-75"
          >
            <Text UNSAFE_style={{ opacity: 0.75 }}>{k}</Text>
            <Text
              UNSAFE_style={{ fontVariantNumeric: 'tabular-nums', wordBreak: 'break-all' }}
            >
              {v ?? '—'}
            </Text>
          </Flex>
        ))}
      </View>
    </Panel>
  )
}
