/* The visual identity of the eleven classes.
 *
 * Colours are the Warcraft class colours players already read fluently —
 * a Mage row is cyan everywhere in the app, so you find your Mage games
 * before you have read a single word. `ink` is the text colour that
 * survives on a dark panel; a few of the class colours (Priest white,
 * Rogue yellow) are too bright to sit under body text unchanged.
 */
export const CLASSES = {
  DEATHKNIGHT: { color: '#6bb6d6', ink: '#8fcbe4', crest: 'rune', focal: '50% 18%' },
  DEMONHUNTER: { color: '#a330c9', ink: '#c46fe0', crest: 'glaive', focal: '52% 20%' },
  DRUID: { color: '#ff7c0a', ink: '#ff9d44', crest: 'antler', focal: '50% 26%' },
  HUNTER: { color: '#aad372', ink: '#bfe08e', crest: 'bow', focal: '54% 20%' },
  MAGE: { color: '#3fc7eb', ink: '#6fd8f2', crest: 'flame', focal: '50% 28%' },
  PALADIN: { color: '#f48cba', ink: '#f7a8cb', crest: 'hammer', focal: '50% 26%' },
  PRIEST: { color: '#e8e8e8', ink: '#f0f0f0', crest: 'candle', focal: '50% 28%' },
  ROGUE: { color: '#fff468', ink: '#fff89a', crest: 'dagger', focal: '50% 26%' },
  SHAMAN: { color: '#3b7fd4', ink: '#6ba3e6', crest: 'totem', focal: '50% 30%' },
  WARLOCK: { color: '#8788ee', ink: '#a5a6f3', crest: 'skull', focal: '50% 26%' },
  WARRIOR: { color: '#c69b6d', ink: '#d6b48f', crest: 'sword', focal: '50% 30%' },
}

export const CLASS_KEYS = Object.keys(CLASSES).sort()

const NEUTRAL = { color: '#8d8d8d', ink: '#b0b0b0', crest: 'sword', focal: '50% 28%' }


/* Where the hero's face sits in their 512×512 art.
 *
 * The banner crops a wide block out of a square, so one shared focal
 * point puts the Lich King's chin off the top while Thrall floats. Eleven
 * numbers, checked once against all eleven arts, solve that without
 * anyone having to prepare eleven cropped images. */
export function classFocal(cls) {
  return classOf(cls).focal
}

export function classOf(cls) {
  return CLASSES[cls] || NEUTRAL
}

export function classColor(cls) {
  return classOf(cls).color
}

/** A translucent wash of the class colour, for panel backgrounds. */
export function classWash(cls, alpha = 0.14) {
  const hex = classColor(cls).replace('#', '')
  const n = parseInt(hex, 16)
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`
}

/** Hero art is filed by class name — see `scripts/fetch_art.py`. */
export function heroArt(cls) {
  return CLASSES[cls] ? `/api/art/hero/${cls}` : null
}

export function tileArt(cardId) {
  return cardId ? `/api/art/tile/${encodeURIComponent(cardId)}` : null
}
