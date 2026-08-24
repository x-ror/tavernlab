import { Item, Picker } from '@adobe/react-spectrum'
import { useApp } from '../store'
import { useT } from '../i18n'

export default function LangPicker() {
  const { t } = useT()
  const { settings, saveSettings } = useApp()
  return (
    <Picker
      aria-label={t('settings.language')}
      selectedKey={settings?.language || 'auto'}
      onSelectionChange={(k) => saveSettings({ language: k === 'auto' ? '' : String(k) })}
      width="size-1700"
    >
      <Item key="auto">{t('settings.lang_auto')}</Item>
      <Item key="uk">{t('settings.lang_uk')}</Item>
      <Item key="en">{t('settings.lang_en')}</Item>
    </Picker>
  )
}
