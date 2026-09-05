/* The visual identity of the eleven classes.
 *
 * Colours are the Warcraft class colours players already read fluently —
 * a Mage row is cyan everywhere in the app, so you find your Mage games
 * before you have read a single word. `ink` is the text colour that
 * survives on a dark panel; a few of the class colours (Priest white,
 * Rogue yellow) are too bright to sit under body text unchanged.
 */
export const CLASSES = {
  DEATHKNIGHT: { color: '#6bb6d6', ink: '#8fcbe4', crest: 'rune' },
  DEMONHUNTER: { color: '#a330c9', ink: '#c46fe0', crest: 'glaive' },
  DRUID: { color: '#ff7c0a', ink: '#ff9d44', crest: 'antler' },
  HUNTER: { color: '#aad372', ink: '#bfe08e', crest: 'bow' },
  MAGE: { color: '#3fc7eb', ink: '#6fd8f2', crest: 'flame' },
  PALADIN: { color: '#f48cba', ink: '#f7a8cb', crest: 'hammer' },
  PRIEST: { color: '#e8e8e8', ink: '#f0f0f0', crest: 'candle' },
  ROGUE: { color: '#fff468', ink: '#fff89a', crest: 'dagger' },
  SHAMAN: { color: '#3b7fd4', ink: '#6ba3e6', crest: 'totem' },
  WARLOCK: { color: '#8788ee', ink: '#a5a6f3', crest: 'skull' },
  WARRIOR: { color: '#c69b6d', ink: '#d6b48f', crest: 'sword' },
}

export const CLASS_KEYS = Object.keys(CLASSES).sort()

const NEUTRAL = { color: '#8d8d8d', ink: '#b0b0b0', crest: 'sword' }

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
