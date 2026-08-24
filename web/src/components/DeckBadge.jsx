import { useEffect, useState } from 'react'
import {
  ActionButton,
  Button,
  ButtonGroup,
  Content,
  Dialog,
  DialogTrigger,
  Divider,
  Flex,
  Heading,
  StatusLight,
  Text,
  TextArea,
} from '@adobe/react-spectrum'
import { useApp } from '../store'
import { useT } from '../i18n'
import ClassCrest from './ClassCrest'
import { deckProblem, formatName } from '../format'

// StatusLight owns its own row of layout, which is wrong inside a
// button; the state still has to be visible, so it is a dot.
const DOT = {
  positive: 'var(--tl-pos)',
  negative: 'var(--tl-neg)',
  notice: '#e68619',
  neutral: 'var(--tl-muted)',
}

/* The deck is app-wide state, so it is edited from the header and read
 * by four different tabs. In the old UI the same code had to be pasted
 * into the rating, mulligan and coach screens separately. */
export default function DeckBadge() {
  const { t } = useT()
  const { deckCode, setDeckCode, deckInfo, hasTelemetry, deckName } = useApp()

  const cls = deckInfo?.cls ? t(`class.${deckInfo.cls}`) : null
  const label = deckCode
    ? cls
      ? deckInfo.name || deckName || t('ui.deck.ok', { cls, n: deckInfo.total ?? '?' })
      : deckInfo?.pending
        ? t('ui.deck.resolving')
        : t('ui.deck.change')
    : t('ui.deck.none')

  const variant = !deckCode
    ? 'neutral'
    : deckInfo?.pending
      ? 'neutral'
      : deckInfo?.ok
        ? hasTelemetry
          ? 'positive'
          : 'notice'
        : 'negative'

  return (
    <DialogTrigger type="popover">
      <ActionButton>
        <Flex direction="row" alignItems="center" gap="size-100">
          {deckInfo?.cls ? (
            <ClassCrest cls={deckInfo.cls} size={16} />
          ) : (
            <span
              aria-hidden="true"
              style={{
                width: 8,
                height: 8,
                borderRadius: '50%',
                flex: '0 0 auto',
                background: DOT[variant],
              }}
            />
          )}
          <Text>{label}</Text>
        </Flex>
      </ActionButton>
      {(close) => <DeckDialog close={close} deckCode={deckCode} setDeckCode={setDeckCode} deckInfo={deckInfo} hasTelemetry={hasTelemetry} />}
    </DialogTrigger>
  )
}

function DeckDialog({ close, deckCode, setDeckCode, deckInfo, hasTelemetry }) {
  const { t } = useT()
  const [draft, setDraft] = useState(deckCode)
  useEffect(() => setDraft(deckCode), [deckCode])

  return (
    <Dialog width="size-6000">
      <Heading>{t('ui.deck.dialog')}</Heading>
      <Divider />
      <Content>
        <TextArea
          label={t('deck.label')}
          placeholder={t('deck.ph')}
          value={draft}
          onChange={setDraft}
          width="100%"
          height="size-1200"
        />
        {deckCode && deckInfo && !deckInfo.pending && (
          <Flex direction="column" gap="size-100" marginTop="size-200">
            {deckInfo.ok ? (
              <StatusLight variant={hasTelemetry ? 'positive' : 'notice'}>
                {formatName(deckInfo.format, t)} ·{' '}
                {hasTelemetry ? t('ui.deck.analysed') : t('ui.deck.not_analysed')}
              </StatusLight>
            ) : (
              <Text UNSAFE_style={{ color: 'var(--tl-neg)' }}>
                {deckProblem(deckInfo, t)}
              </Text>
            )}
          </Flex>
        )}
      </Content>
      <ButtonGroup>
        <Button variant="secondary" onPress={close}>
          {t('ui.deck.cancel')}
        </Button>
        <Button
          variant="accent"
          isDisabled={draft.trim() === deckCode.trim()}
          onPress={() => {
            setDeckCode(draft.trim())
            close()
          }}
        >
          {t('ui.deck.save')}
        </Button>
      </ButtonGroup>
    </Dialog>
  )
}
