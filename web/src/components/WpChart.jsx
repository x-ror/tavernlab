import { useId, useMemo } from 'react'
import { Text } from '@adobe/react-spectrum'
import { useT } from '../i18n'

/* Hand-drawn SVG on purpose: a charting library would be the single
 * biggest dependency in the tree, and this chart has exactly one job.
 *
 * The hatching is not decoration. Every point comes from `logistic_v1`
 * (9 weights, 66.7% on simulator snapshots, uncalibrated on human games),
 * so the line must never read as a measured curve. It is hatched for its
 * whole length, and no point is ever coloured "bad".
 */
export default function WpChart({ series, activeSeq, onPick, height = 160 }) {
  const { t } = useT()
  const uid = useId().replace(/:/g, '')
  const pts = useMemo(() => (series || []).filter((p) => typeof p.wp === 'number'), [series])

  if (!pts.length) return <Text>{t('review.no_wp')}</Text>

  const W = 800
  const H = height
  const padL = 28
  const padB = 18
  const x = (i) => padL + (i * (W - padL - 8)) / Math.max(1, pts.length - 1)
  const y = (wp) => 6 + (1 - wp) * (H - padB - 6)

  const line = pts.map((p, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(p.wp).toFixed(1)}`).join(' ')
  const area = `${line} L${x(pts.length - 1).toFixed(1)},${y(0).toFixed(1)} L${x(0).toFixed(1)},${y(0).toFixed(1)} Z`
  const activeIdx = pts.findIndex((p) => p.seq === activeSeq)

  // Turn boundaries, so the eye can find "turn 7" without a legend.
  const marks = []
  let prevTurn = null
  pts.forEach((p, i) => {
    if (p.turn !== prevTurn) {
      marks.push({ i, turn: p.turn })
      prevTurn = p.turn
    }
  })

  return (
    <div style={{ width: '100%' }}>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        style={{ width: '100%', height: 'auto', display: 'block', touchAction: 'none' }}
        role="img"
        aria-label={t('review.wp_title')}
      >
        <defs>
          <pattern id={`hatch-${uid}`} width="6" height="6" patternTransform="rotate(45)" patternUnits="userSpaceOnUse">
            <rect width="6" height="6" fill="var(--tl-wp-fill)" />
            <line x1="0" y1="0" x2="0" y2="6" stroke="var(--tl-wp-hatch)" strokeWidth="2" />
          </pattern>
        </defs>

        <line x1={padL} y1={y(0.5)} x2={W - 8} y2={y(0.5)} stroke="var(--tl-grid)" strokeDasharray="4 4" />
        <text x={2} y={y(0.5) + 4} fontSize="10" fill="var(--tl-muted)">50%</text>
        <text x={2} y={y(1) + 8} fontSize="10" fill="var(--tl-muted)">100</text>
        <text x={6} y={y(0)} fontSize="10" fill="var(--tl-muted)">0</text>

        {marks.map((m) => (
          <g key={m.i}>
            <line x1={x(m.i)} y1={4} x2={x(m.i)} y2={H - padB} stroke="var(--tl-grid)" strokeWidth="1" opacity="0.5" />
            <text x={x(m.i) + 2} y={H - 6} fontSize="9" fill="var(--tl-muted)">
              {m.turn}
            </text>
          </g>
        ))}

        <path d={area} fill={`url(#hatch-${uid})`} opacity="0.85" />
        <path d={line} fill="none" stroke="var(--tl-wp-line)" strokeWidth="2" strokeDasharray="5 3" />

        {activeIdx >= 0 && (
          <g>
            <line
              x1={x(activeIdx)}
              y1={4}
              x2={x(activeIdx)}
              y2={H - padB}
              stroke="var(--tl-accent)"
              strokeWidth="2"
            />
            <circle cx={x(activeIdx)} cy={y(pts[activeIdx].wp)} r="4" fill="var(--tl-accent)" />
          </g>
        )}

        {onPick &&
          pts.map((p, i) => (
            <rect
              key={p.seq ?? i}
              x={x(i) - 4}
              y={0}
              width={8}
              height={H}
              fill="transparent"
              style={{ cursor: 'pointer' }}
              onClick={() => onPick(p)}
            >
              <title>{`${t('review.turn_n', { n: p.turn })} — ${t('replay.wp', { v: Math.round(p.wp * 100) })}`}</title>
            </rect>
          ))}
      </svg>
    </div>
  )
}
