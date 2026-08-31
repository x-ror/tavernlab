import { useState } from 'react'
import {
  ActionButton,
  Flex,
  Item,
  Picker,
  Text,
  TextArea,
  TextField,
} from '@adobe/react-spectrum'
import { post, runJob } from '../api'
import { useT } from '../i18n'
import { Panel } from '../components/ui'
import { CLASS_KEYS } from '../classes'
import { pct } from '../format'

/* The Arena draft tab: type your picks, type the offer, get two answers.
 *
 * The cheap one is counters — curve, Taunt, weapons, the text-read
 * approximations — which never simulate and never lie. The measured one
 * completes your draft with a random tail from the season pool and plays
 * each candidate against the generated Arena field; the number it prints
 * is a comparison *between the three cards on this draft*, never a season
 * winrate, and the answer says how much of the simulated deck was real
 * picks. No tier list is imported and none is shown: that would be
 * someone else's cloud wearing our interface (DESIGN.md U24/U28).
 */
export default function Arena() {
  const { t } = useT()
  const [cls, setCls] = useState('MAGE')
  const [picked, setPicked] = useState('')
  const [c1, setC1] = useState('')
  const [c2, setC2] = useState('')
  const [c3, setC3] = useState('')
  const [counters, setCounters] = useState(null)
  const [result, setResult] = useState(null)
  const [progress, setProgress] = useState([])
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)

  const pickedList = () =>
    picked
      .split('\n')
      .map((s) => s.trim())
      .filter(Boolean)
  const candidates = () => [c1, c2, c3].map((s) => s.trim()).filter(Boolean)

  async function refresh() {
    setErr('')
    try {
      setCounters(
        await post('/api/arena/draft', {
          class: cls,
          picked: pickedList(),
          candidates: candidates(),
        }),
      )
    } catch (e) {
      setErr(String(e.message || e))
    }
  }

  async function compare() {
    setErr('')
    setResult(null)
    setProgress([])
    setBusy(true)
    try {
      const r = await runJob(
        '/api/arena/pick',
        { class: cls, picked: pickedList(), candidates: candidates() },
        setProgress,
      )
      r.scores.sort((a, b) => (b.winrate ?? -1) - (a.winrate ?? -1))
      setResult(r)
    } catch (e) {
      setErr(String(e.message || e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Flex direction="column" gap="size-300">
      <Panel title={t('arena.title')}>
        <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.85rem' }}>
          {t('arena.blurb')}
        </Text>
        <Flex direction="row" gap="size-200" alignItems="end" wrap marginTop="size-200">
          <Picker
            label={t('arena.class')}
            items={CLASS_KEYS.map((c) => ({ id: c, name: t(`class.${c}`) }))}
            selectedKey={cls}
            onSelectionChange={(k) => setCls(String(k))}
            width="size-2000"
          >
            {(item) => <Item>{item.name}</Item>}
          </Picker>
          <TextArea
            label={t('arena.picked')}
            description={t('arena.picked_hint')}
            value={picked}
            onChange={setPicked}
            width="size-4600"
            height="size-2000"
          />
          <Flex direction="column" gap="size-100">
            <TextField label={t('arena.candidates')} value={c1} onChange={setC1} width="size-3000" />
            <TextField aria-label={t('arena.candidates')} value={c2} onChange={setC2} width="size-3000" />
            <TextField aria-label={t('arena.candidates')} value={c3} onChange={setC3} width="size-3000" />
          </Flex>
          <Flex direction="column" gap="size-100">
            <ActionButton onPress={refresh}>{t('arena.count_btn')}</ActionButton>
            <ActionButton onPress={compare} isDisabled={busy || candidates().length < 2}>
              {t('arena.compare_btn')}
            </ActionButton>
          </Flex>
        </Flex>
        {err && (
          <Text UNSAFE_style={{ color: 'var(--tl-bad, #e66)', fontSize: '0.85rem' }} marginTop="size-100">
            {err}
          </Text>
        )}
      </Panel>

      {counters && <Counters t={t} c={counters} onCut={(name) => {
        setPicked((prev) =>
          prev
            .split('\n')
            .map((s) => s.trim())
            .filter((s) => s && s !== name)
            .join('\n'),
        )
      }} />}

      {busy && progress.length > 0 && (
        <Panel title={t('arena.compare_head')}>
          {progress.map((line, i) => (
            <div key={i} style={{ color: 'var(--tl-muted)', fontSize: '0.8rem' }}>
              {line}
            </div>
          ))}
        </Panel>
      )}

      {result && <Scores t={t} r={result} />}
    </Flex>
  )
}

function Counters({ t, c, onCut }) {
  const rows = [
    ['two_drop_minions', t('arena.two_drops')],
    ['taunts', t('arena.taunts')],
    ['weapons', t('arena.weapons')],
    ['hard_removal', t('arena.removal')],
    ['damage_spells', t('arena.damage_spells')],
    ['aoe', t('arena.aoe')],
    ['draw', t('arena.draw')],
  ]
  const max = Math.max(1, ...c.curve)
  const runes = c.runes || [0, 0, 0]
  return (
    <Panel title={t('arena.counters_head', { n: c.picked, total: c.deck_size })}>
      <Flex direction="row" gap="size-500" wrap>
        <div>
          <div style={{ color: 'var(--tl-muted)', fontSize: '0.8rem', marginBottom: 6 }}>
            {t('arena.curve')}
          </div>
          <Flex direction="row" gap="size-50" alignItems="end">
            {c.curve.map((n, cost) => (
              <div key={cost} style={{ textAlign: 'center', width: 26 }}>
                <div
                  style={{
                    height: 8 + (n / max) * 52,
                    background: n > 0 ? 'var(--tl-accent, #d6a44a)' : 'var(--tl-line)',
                    borderRadius: 2,
                  }}
                  title={String(n)}
                />
                <div style={{ fontSize: '0.7rem', color: 'var(--tl-muted)' }}>
                  {cost === 7 ? '7+' : cost}
                </div>
                <div style={{ fontSize: '0.75rem' }}>{n}</div>
              </div>
            ))}
          </Flex>
        </div>
        <div style={{ fontSize: '0.85rem' }}>
          {rows.map(([k, label]) => (
            <div key={k}>
              {label}: <b>{c[k]}</b>
            </div>
          ))}
          {(runes[0] > 0 || runes[1] > 0 || runes[2] > 0) && (
            <div>
              {t('arena.runes')}: <b>{runes[0]}🩸 {runes[1]}❄ {runes[2]}☠</b>
            </div>
          )}
        </div>
      </Flex>
      <Text
        UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.75rem' }}
        marginTop="size-150"
      >
        {t('arena.counters_note')}
      </Text>
      {c.unimplemented_picked.length > 0 && (
        <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.8rem' }} marginTop="size-100">
          {t('arena.unimpl', { cards: c.unimplemented_picked.join(', ') })}
        </Text>
      )}
      {c.candidates?.some((x) => x.group?.length) && (
        <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.8rem' }} marginTop="size-100">
          {c.candidates
            .filter((x) => x.group?.length)
            .map((x) => t('arena.group', { card: x.name, pack: x.group.join(', ') }))
            .join(' · ')}
        </Text>
      )}
      {c.cuts?.length > 0 && (
        <div style={{ marginTop: 12 }}>
          <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.8rem' }}>
            {t('arena.cuts_note')}
          </Text>
          <Flex direction="row" gap="size-100" wrap marginTop="size-75">
            {c.cuts.map((name) => (
              <ActionButton key={name} onPress={() => onCut?.(name)}>
                {t('arena.cut', { card: name })}
              </ActionButton>
            ))}
          </Flex>
        </div>
      )}
    </Panel>
  )
}

function Scores({ t, r }) {
  const best = r.scores[0]?.winrate ?? null
  return (
    <Panel title={t('arena.scores_head')}>
      <table className="tl-table" style={{ fontSize: '0.9rem' }}>
        <tbody>
          {r.scores.map((s) => (
            <tr key={s.card}>
              <td style={{ paddingRight: 16 }}>
                {s.card}
                {s.group?.length > 0 && (
                  <div style={{ color: 'var(--tl-muted)', fontSize: '0.75rem' }}>
                    {t('arena.group', { card: s.card, pack: s.group.join(', ') })}
                  </div>
                )}
              </td>
              <td style={{ paddingRight: 16 }}>
                <b>{s.winrate === null ? '—' : pct(s.winrate, 1)}</b>
              </td>
              <td style={{ paddingRight: 16, color: 'var(--tl-muted)' }}>
                {s.winrate !== null && best !== null && s.winrate !== best
                  ? t('arena.behind', { d: ((best - s.winrate) * 100).toFixed(1) })
                  : ''}
              </td>
              <td style={{ color: 'var(--tl-muted)' }}>
                {t('arena.games', { n: s.games })}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <Text
        UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.8rem' }}
        marginTop="size-150"
      >
        {t('arena.real', { n: r.real_cards, total: r.deck_size })}
      </Text>
      {r.dropped_picked.length > 0 && (
        <Text UNSAFE_style={{ color: 'var(--tl-muted)', fontSize: '0.8rem' }} marginTop="size-50">
          {t('arena.dropped', { cards: r.dropped_picked.join(', ') })}
        </Text>
      )}
    </Panel>
  )
}
