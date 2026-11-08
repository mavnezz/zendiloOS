/**
 * The desktop backgrounds zendiloOS ships with: one image and gradients,
 * which cost nothing to ship and stay sharp at any resolution, and one of
 * a single colour the account picks. Names are proper names, the same in
 * every language; the single colour's is the colour itself.
 */
export interface Wallpaper {
  /** Kept in the account's preferences. */
  id: string
  name: string
  /** Value for `background-image`. */
  image: string
  /** Shown underneath: while an image loads, and through a gradient's gaps. */
  color: string
  /** Whether it is light enough to need dark icon labels. */
  tone: 'dark' | 'light'
}

export const WALLPAPERS: readonly Wallpaper[] = [
  {
    id: 'aurora',
    name: 'Aurora',
    image: 'url("/wallpapers/aurora.png")',
    color: '#0b1428',
    tone: 'dark',
  },
  {
    id: 'abyss',
    name: 'Abyss',
    color: '#060d1c',
    tone: 'dark',
    image: [
      'radial-gradient(90% 70% at 8% 6%, #1f4f9e 0%, transparent 62%)',
      'radial-gradient(70% 60% at 92% 88%, #0d5f66 0%, transparent 64%)',
      'radial-gradient(60% 55% at 62% 26%, #2c2a7d 0%, transparent 70%)',
    ].join(', '),
  },
  {
    id: 'nebula',
    name: 'Nebula',
    color: '#100a24',
    tone: 'dark',
    image: [
      'radial-gradient(85% 65% at 78% 12%, #6d3ba8 0%, transparent 62%)',
      'radial-gradient(75% 60% at 12% 82%, #2b3f9c 0%, transparent 66%)',
      'radial-gradient(55% 50% at 45% 48%, #a33c7a 0%, transparent 72%)',
    ].join(', '),
  },
  {
    id: 'tide',
    name: 'Tide',
    color: '#04201f',
    tone: 'dark',
    image: [
      'radial-gradient(85% 70% at 16% 14%, #12716b 0%, transparent 64%)',
      'radial-gradient(70% 60% at 88% 80%, #14508c 0%, transparent 66%)',
      'radial-gradient(50% 45% at 58% 40%, #1d8f6a 0%, transparent 72%)',
    ].join(', '),
  },
  {
    id: 'graphite',
    name: 'Graphite',
    color: '#15171c',
    tone: 'dark',
    image: [
      'radial-gradient(80% 60% at 10% 8%, #3a4250 0%, transparent 62%)',
      'radial-gradient(70% 60% at 90% 92%, #23282f 0%, transparent 66%)',
    ].join(', '),
  },
  {
    id: 'daybreak',
    name: 'Daybreak',
    color: '#dce6f5',
    tone: 'light',
    image: [
      'radial-gradient(85% 70% at 12% 10%, #a8c8f0 0%, transparent 64%)',
      'radial-gradient(70% 60% at 88% 86%, #f0cfd6 0%, transparent 66%)',
      'radial-gradient(55% 50% at 60% 40%, #cbe0f2 0%, transparent 72%)',
    ].join(', '),
  },
]

export const DEFAULT_WALLPAPER = WALLPAPERS[0]!

/** The background of one colour, as the account's preferences name it. */
export const SOLID_WALLPAPER = 'color'

/** Colours offered beside the picker: calm ones that suit the desktop. */
export const SOLID_COLORS = [
  { id: 'slate', color: '#1e293b' },
  { id: 'navy', color: '#1b2a4e' },
  { id: 'teal', color: '#134e4a' },
  { id: 'aubergine', color: '#3b1f3a' },
  { id: 'warmGrey', color: '#5c5650' },
  { id: 'lightGrey', color: '#dfe2e6' },
] as const

export const DEFAULT_SOLID_COLOR: string = SOLID_COLORS[0].color

/** `#rrggbb` in lowercase, as the colour picker gives it and the server keeps it. */
export function isHexColor(value: string | null | undefined): value is string {
  return typeof value === 'string' && /^#[0-9a-f]{6}$/.test(value)
}

/** Relative luminance of `#rrggbb` (WCAG 2): 0 for black, 1 for white. */
export function luminance(color: string): number {
  const channel = (at: number) => {
    const value = parseInt(color.slice(at, at + 2), 16) / 255
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
}

/**
 * Whether text on `color` reads better dark or light: whichever of black
 * and white contrasts more with it. They contrast equally at a luminance
 * of about 0.18.
 */
export function toneOf(color: string): Wallpaper['tone'] {
  const level = luminance(color)
  return (level + 0.05) / 0.05 > 1.05 / (level + 0.05) ? 'light' : 'dark'
}

/** The background of one colour; one that is not `#rrggbb` gives the default colour. */
export function solidWallpaper(color: string | null | undefined): Wallpaper {
  const valid = isHexColor(color) ? color : DEFAULT_SOLID_COLOR
  return { id: SOLID_WALLPAPER, name: valid, image: 'none', color: valid, tone: toneOf(valid) }
}

/** The background named `id`; `color` is the one of the background of one colour. */
export function findWallpaper(id: string | null | undefined, color?: string | null): Wallpaper {
  if (id === SOLID_WALLPAPER) return solidWallpaper(color)
  return WALLPAPERS.find((wallpaper) => wallpaper.id === id) ?? DEFAULT_WALLPAPER
}

const STORAGE_KEY = 'zendiloos.wallpaper'

/** Puts `wallpaper` behind the desktop and the sign-in pages, and remembers it in this browser. */
export function applyWallpaper(wallpaper: Wallpaper): void {
  const root = document.documentElement.style
  root.setProperty('--wallpaper-image', wallpaper.image)
  root.setProperty('--wallpaper-color', wallpaper.color)
  document.documentElement.dataset.wallpaperTone = wallpaper.tone
  try {
    // A colour is kept as itself: names never start with `#`.
    localStorage.setItem(
      STORAGE_KEY,
      wallpaper.id === SOLID_WALLPAPER ? wallpaper.color : wallpaper.id,
    )
  } catch {
    // Applies to this page only.
  }
}

/** The background this browser showed last, before the account's is known. */
export function storedWallpaper(): Wallpaper {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    return isHexColor(stored) ? solidWallpaper(stored) : findWallpaper(stored)
  } catch {
    return DEFAULT_WALLPAPER
  }
}
