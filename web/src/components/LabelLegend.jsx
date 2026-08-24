import { Badge, Flex, Text, View } from '@adobe/react-spectrum'
import { useT } from '../i18n'

/* The greyed half of this legend is the point, not an oversight: the
 * gates live in `eval/classify.py`, and a label that has not passed them
 * is shown as unavailable with the gate named, rather than quietly
 * omitted (README, "Honesty of the evaluation"). */
export default function LabelLegend({ legend }) {
  const { t } = useT()
  if (!legend?.length) return <Text>{t('review.legend_missing')}</Text>

  return (
    <Flex direction="column" gap="size-150">
      <Text UNSAFE_style={{ fontSize: '.85rem', opacity: 0.8 }}>{t('review.legend_help')}</Text>
      {legend.map((l) => (
        <Flex key={l.key} direction="row" gap="size-150" alignItems="center" wrap>
          <View minWidth="size-1700">
            <Badge variant={l.available ? 'info' : 'neutral'}>{t(`label.${l.key}`)}</Badge>
          </View>
          <Text UNSAFE_style={{ fontSize: '.8rem', opacity: l.available ? 0.9 : 0.6 }}>
            {l.available ? t('review.legend_active') : t('review.legend_soon')}
            {!l.available && l.needs?.length ? ` — ${t('review.legend_needs', { gates: l.needs.join(', ') })}` : ''}
            {l.note ? ` · ${l.note}` : ''}
          </Text>
        </Flex>
      ))}
    </Flex>
  )
}
