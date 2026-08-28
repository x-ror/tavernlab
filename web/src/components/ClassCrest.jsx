import { classOf } from '../classes'

/* Hand-drawn class sigils.
 *
 * These are ours, so they always render — no download, no licence, no
 * broken-image box. Card and hero art is a cached bonus on top; the
 * crest is the floor the interface never falls below.
 */
const PATHS = {
  // Warrior: a blade, point up, with a crossguard.
  sword: 'M12 2 L14 7 L14 15 L10 15 L10 7 Z M7 15 H17 L16 17 H8 Z M11 17 H13 V22 H11 Z',
  // Shaman: a stacked totem.
  totem:
    'M6 3 H18 L16.5 7 H7.5 Z M7.5 8.5 H16.5 L15.5 12.5 H8.5 Z M8.5 14 H15.5 L14.5 18 H9.5 Z M10 19.5 H14 L13 22 H11 Z',
  // Rogue: a curved dagger.
  dagger:
    'M13 2 L15 5 L13.5 12 H10.5 L9 5 Z M7.5 13 H16.5 L15.5 15 H8.5 Z M11 16 H13 L12 22 Z',
  // Paladin: a war hammer.
  hammer: 'M5 4 H19 V10 H5 Z M8 10.5 H16 L15 13 H9 Z M11 13.5 H13 V22 H11 Z',
  // Hunter: a drawn bow.
  bow: 'M8 2 C15 6 15 18 8 22 L9.8 21 C15.6 17 15.6 7 9.8 3 Z M7 2.5 L7 21.5 L5.6 21.5 L5.6 2.5 Z M16 10.5 H21 L18.5 12 L21 13.5 H16 Z',
  // Druid: an antler.
  antler:
    'M12 22 V12 M12 12 C12 8 9 7 7 4 M7 4 L5 7 M7 4 L9.5 3 M12 12 C12 8 15 7 17 4 M17 4 L19 7 M17 4 L14.5 3 M12 16 C12 14 10 13 8 12 M12 16 C12 14 14 13 16 12',
  // Mage: a flame.
  flame:
    'M12 2 C13.5 6 17 7.5 17 12.5 C17 16.6 14.8 19 12 19 C9.2 19 7 16.6 7 12.5 C7 9.5 9 8 9.5 5.5 C10.6 7 11 8 11 9.5 C11.8 8 12.2 5.2 12 2 Z',
  // Warlock: a skull.
  skull:
    'M12 2 C7.6 2 4.5 5.2 4.5 9.6 C4.5 12.3 5.8 14 7 15 V18 H17 V15 C18.2 14 19.5 12.3 19.5 9.6 C19.5 5.2 16.4 2 12 2 Z M9 9.5 A1.7 1.7 0 1 0 9 9.49 Z M15 9.5 A1.7 1.7 0 1 0 15 9.49 Z M8 19.5 H10 V22 H8 Z M11 19.5 H13 V22 H11 Z M14 19.5 H16 V22 H14 Z',
  // Priest: a candle with a halo.
  candle:
    'M12 2 C13 4 14 5 14 6.6 C14 8 13.1 9 12 9 C10.9 9 10 8 10 6.6 C10 5 11 4 12 2 Z M8.5 10.5 H15.5 V22 H8.5 Z M4 12 H7 M17 12 H20',
  // Demon Hunter: paired warglaives.
  glaive:
    'M4 3 C9 5 12 9 12 13 L12 21 L10.5 21 L10.5 13 C10.5 9.6 8 6.4 4 4.6 Z M20 3 C15 5 12 9 12 13 L12 21 L13.5 21 L13.5 13 C13.5 9.6 16 6.4 20 4.6 Z M3 2 L6 3.4 L4.4 5 Z M21 2 L18 3.4 L19.6 5 Z',
  // Death Knight: a rune sigil.
  rune: 'M12 2 L20 7 V17 L12 22 L4 17 V7 Z M12 6 V18 M8 9 L16 15 M16 9 L8 15',
}

const STROKED = new Set(['antler', 'candle', 'rune'])

export default function ClassCrest({ cls, size = 20, color, title, className, style, ...rest }) {
  const meta = classOf(cls)
  const tone = color || meta.color
  const d = PATHS[meta.crest] || PATHS.sword
  const stroked = STROKED.has(meta.crest)

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      role={title ? 'img' : 'presentation'}
      aria-label={title || undefined}
      aria-hidden={title ? undefined : 'true'}
      className={className}
      style={{ flex: '0 0 auto', display: 'block', ...style }}
      {...rest}
    >
      {title && <title>{title}</title>}
      <path
        d={d}
        fill={stroked ? 'none' : tone}
        stroke={tone}
        strokeWidth={stroked ? 1.6 : 0.8}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}
