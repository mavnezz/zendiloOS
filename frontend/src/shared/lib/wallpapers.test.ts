import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  applyWallpaper,
  DEFAULT_SOLID_COLOR,
  DEFAULT_WALLPAPER,
  findWallpaper,
  isHexColor,
  luminance,
  SOLID_COLORS,
  solidWallpaper,
  storedWallpaper,
  toneOf,
  WALLPAPERS,
} from './wallpapers'

afterEach(() => localStorage.clear())

describe('a solid colour', () => {
  it('is #rrggbb in lowercase', () => {
    expect(isHexColor('#1e293b')).toBe(true)
    for (const bad of ['#1E293B', '1e293b', '#1e293', '#1e293b0', '#1e29gb', '', null, undefined]) {
      expect(isHexColor(bad), String(bad)).toBe(false)
    }
  })

  it('is as bright as WCAG measures it', () => {
    expect(luminance('#000000')).toBe(0)
    expect(luminance('#ffffff')).toBeCloseTo(1)
    expect(luminance('#808080')).toBeCloseTo(0.216, 3)
    expect(luminance('#ff0000')).toBeCloseTo(0.2126)
  })

  it('takes the text that contrasts more: dark on light, light on dark', () => {
    expect(toneOf('#ffffff')).toBe('light')
    expect(toneOf('#000000')).toBe('dark')
    expect(toneOf('#757575')).toBe('dark')
    expect(toneOf('#767676')).toBe('light')
    expect(toneOf('#ffff00'), 'yellow').toBe('light')
    expect(toneOf('#0000ff'), 'blue').toBe('dark')
    const tones = SOLID_COLORS.map((swatch) => toneOf(swatch.color))
    expect(tones).toEqual(['dark', 'dark', 'dark', 'dark', 'dark', 'light'])
  })

  it('is a background of its own, the default colour for one that is not valid', () => {
    expect(solidWallpaper('#dfe2e6')).toEqual({
      id: 'color',
      name: '#dfe2e6',
      image: 'none',
      color: '#dfe2e6',
      tone: 'light',
    })
    expect(solidWallpaper('red').color).toBe(DEFAULT_SOLID_COLOR)
    expect(findWallpaper('color', '#134e4a').color).toBe('#134e4a')
    expect(findWallpaper('color', null).color).toBe(DEFAULT_SOLID_COLOR)
    expect(findWallpaper('tide', '#134e4a')).toBe(WALLPAPERS[3])
  })

  it('is remembered in this browser as the colour itself', () => {
    applyWallpaper(solidWallpaper('#dfe2e6'))
    expect(localStorage.getItem('zendiloos.wallpaper')).toBe('#dfe2e6')
    expect(document.documentElement.style.getPropertyValue('--wallpaper-image')).toBe('none')
    expect(document.documentElement.dataset.wallpaperTone).toBe('light')
    expect(storedWallpaper()).toEqual(solidWallpaper('#dfe2e6'))

    applyWallpaper(DEFAULT_WALLPAPER)
    expect(storedWallpaper()).toBe(DEFAULT_WALLPAPER)
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => '#ABCDEF')
    expect(storedWallpaper(), 'not lowercase, not a colour').toBe(DEFAULT_WALLPAPER)
    vi.restoreAllMocks()
  })
})
