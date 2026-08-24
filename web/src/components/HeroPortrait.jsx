import { useState } from 'react'
import { classOf, classWash, heroArt } from '../classes'
import ClassCrest from './ClassCrest'

/* The gilded medallion the whole interface hangs off.
 *
 * If `scripts/fetch_art.py` has run, this is the real hero portrait
 * behind a brass ring. If it has not, it is the class crest on a wash of
 * the class colour — same size, same weight, no broken image, no layout
 * shift. The art is a bonus, never a dependency.
 */
export default function HeroPortrait({
  cls,
  size = 56,
  dim = false,
  title,
  flip = false,
}) {
  const [failed, setFailed] = useState(false)
  const meta = classOf(cls)
  const src = heroArt(cls)
  const showArt = src && !failed

  return (
    <div
      className="tl-portrait"
      title={title}
      style={{
        width: size,
        height: size,
        '--crest-color': meta.color,
        background: classWash(cls, 0.3),
        opacity: dim ? 0.55 : 1,
      }}
    >
      {showArt ? (
        <img
          src={src}
          alt=""
          onError={() => setFailed(true)}
          style={{ transform: `scale(1.35) ${flip ? 'scaleX(-1)' : ''}` }}
        />
      ) : (
        <ClassCrest cls={cls} size={Math.round(size * 0.5)} />
      )}
      <span className="tl-portrait-ring" aria-hidden="true" />
    </div>
  )
}

/** Two portraits facing each other — the header of every game. */
export function Versus({ us, them, size = 64, children }) {
  return (
    <div className="tl-versus">
      <HeroPortrait cls={us} size={size} title={us} />
      <span className="tl-versus-mark" aria-hidden="true">
        VS
      </span>
      <HeroPortrait cls={them} size={size} title={them} flip />
      {children}
    </div>
  )
}
