import { useMemo } from 'react'
import { Flex, Item, Provider, TabList, Tabs, Text, View, defaultTheme } from '@adobe/react-spectrum'
import { AppProvider, go, useApp, useRoute } from './store'
import { I18nProvider, pickLang, useT } from './i18n'
import DeckBadge from './components/DeckBadge'
import LangPicker from './components/LangPicker'
import CoachHome from './routes/CoachHome'
import Games from './routes/Games'
import GamePage from './routes/GamePage'
import DeckLab from './routes/DeckLab'
import Meta from './routes/Meta'
import Settings from './routes/Settings'

const SECTIONS = ['coach', 'games', 'deck', 'meta', 'settings']

export default function App() {
  return (
    <AppProvider>
      <Localised />
    </AppProvider>
  )
}

function Localised() {
  const { settings } = useApp()
  const lang = pickLang(settings?.language)
  return (
    // Dark is pinned rather than followed from the OS. Every screen here
    // is dominated by Hearthstone art, which was authored on a dark
    // ground; the same layout in Spectrum's light theme puts that art in
    // a white box and looks like a bug.
    <Provider theme={defaultTheme} colorScheme="dark" locale={lang === 'uk' ? 'uk-UA' : 'en-US'}>
      <I18nProvider lang={lang}>
        <Shell />
      </I18nProvider>
    </Provider>
  )
}

function Shell() {
  const { t } = useT()
  const { parts } = useRoute()
  const section = SECTIONS.includes(parts[0]) ? parts[0] : 'coach'

  const games = useMemo(
    () =>
      // #/games/<id>[/review|replay]
      parts[1] ? (
        <GamePage
          gameId={Number(parts[1])}
          view={parts[2] || 'review'}
          seq={parts[3] ? Number(parts[3]) : null}
        />
      ) : (
        <Games />
      ),
    [parts.join('/')],
  )

  return (
    <div className="tl-room" style={{ minHeight: '100vh' }}>
      <header
        style={{
          position: 'sticky',
          top: 0,
          zIndex: 10,
          padding: '16px 28px 0',
          background: 'linear-gradient(180deg, rgba(20,16,14,.97), rgba(20,16,14,.86))',
          backdropFilter: 'blur(8px)',
          borderBottom: '1px solid var(--tl-line)',
        }}
      >
        <div style={{ maxWidth: 1240, margin: '0 auto' }}>
          <Flex
            direction="row"
            alignItems="center"
            justifyContent="space-between"
            gap="size-200"
            wrap
          >
            <Flex direction="column" gap="size-25">
              <div className="tl-brand">
                {t('app.brand_a')}
                <em>{t('app.brand_b')}</em>
              </div>
              <Text UNSAFE_style={{ fontSize: '0.75rem', color: 'var(--tl-muted)' }}>
                {t('ui.shell.tagline')}
              </Text>
            </Flex>
            <Flex direction="row" alignItems="center" gap="size-200" wrap>
              <DeckBadge />
              <LangPicker />
            </Flex>
          </Flex>

          <Tabs
            aria-label={t('app.title')}
            selectedKey={section}
            onSelectionChange={(key) => go(key)}
            marginTop="size-150"
          >
            <TabList>
              <Item key="coach">{t('ui.nav.coach')}</Item>
              <Item key="games">{t('ui.nav.games')}</Item>
              <Item key="deck">{t('ui.nav.deck')}</Item>
              <Item key="meta">{t('ui.nav.tiers')}</Item>
              <Item key="settings">{t('ui.nav.settings')}</Item>
            </TabList>
          </Tabs>
        </div>
      </header>

      <View paddingX="size-350" paddingY="size-400" maxWidth="1240px" marginX="auto">
        {section === 'coach' && <CoachHome />}
        {section === 'games' && games}
        {section === 'deck' && <DeckLab tab={parts[1] || 'rating'} sub={parts[2]} />}
        {section === 'meta' && <Meta />}
        {section === 'settings' && <Settings />}
      </View>
    </div>
  )
}
