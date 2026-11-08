import { beforeEach, describe, expect, it, vi } from 'vitest'

import { mountApp, settle } from '@/desktop/testing/mount'
import { i18n } from '@/shared/i18n'
import { called, on, reset } from '@/shared/test-support/server'

import AppearancePage from './AppearancePage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string) => i18n.global.t(key)
const root = document.documentElement
const saved = () => called('PUT', '/api/v1/preferences').map((call) => call.body)

/** The radio called `name`: a background's tile or a colour's swatch. */
function radio(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll<HTMLButtonElement>('[role="radio"]')].find(
    (button) => button.textContent?.trim() === name || button.getAttribute('aria-label') === name,
  )
  if (!found) throw new Error(`no radio ${name}`)
  return found
}

const picker = () =>
  document.querySelector<HTMLInputElement>(`input[aria-label="${t('appearance.pickColour')}"]`)

beforeEach(() => {
  reset()
  localStorage.clear()
  on('PUT', '/api/v1/preferences', null)
})

describe('the solid background', () => {
  it('is a tile beside the pictures; choosing it shows its colours', async () => {
    await mountApp(AppearancePage)
    expect(picker(), 'only once chosen').toBeNull()

    radio(t('appearance.solid')).click()
    await settle()

    expect(radio(t('appearance.solid')).getAttribute('aria-checked')).toBe('true')
    expect(saved().at(-1)).toMatchObject({ wallpaper: 'color', wallpaper_color: '#1e293b' })
    expect(root.style.getPropertyValue('--wallpaper-color')).toBe('#1e293b')
    expect(root.style.getPropertyValue('--wallpaper-image')).toBe('none')
    expect(picker()?.value).toBe('#1e293b')
    const swatches = ['slate', 'navy', 'teal', 'aubergine', 'warmGrey', 'lightGrey']
    for (const id of swatches) expect(() => radio(t(`appearance.colours.${id}`))).not.toThrow()
  })

  it('takes a swatch at once, and keeps dark labels on a light one', async () => {
    await mountApp(AppearancePage)
    radio(t('appearance.solid')).click()
    await settle()

    radio(t('appearance.colours.lightGrey')).click()
    await settle()

    expect(saved().at(-1)).toMatchObject({ wallpaper: 'color', wallpaper_color: '#dfe2e6' })
    expect(root.dataset.wallpaperTone).toBe('light')
    expect(radio(t('appearance.colours.lightGrey')).getAttribute('aria-checked')).toBe('true')
    const tile = radio(t('appearance.solid'))
    expect(tile.style.backgroundColor).toBe('rgb(223, 226, 230)')
    expect(tile.querySelector('span')?.className).toContain('text-neutral-900')
    expect(radio('Aurora').querySelector('span')?.className).toContain('text-white')
  })

  it('follows the picker while it moves, and keeps what it closes on', async () => {
    await mountApp(AppearancePage)
    radio(t('appearance.solid')).click()
    await settle()
    const before = saved().length

    const input = picker()!
    input.value = '#336699'
    input.dispatchEvent(new Event('input'))
    await settle()
    expect(root.style.getPropertyValue('--wallpaper-color')).toBe('#336699')
    expect(saved(), 'not kept while picking').toHaveLength(before)

    input.dispatchEvent(new Event('change'))
    await settle()
    expect(saved().at(-1)).toMatchObject({ wallpaper: 'color', wallpaper_color: '#336699' })
    expect(root.dataset.wallpaperTone).toBe('dark')

    radio('Tide').click()
    await settle()
    expect(picker()).toBeNull()
    expect(saved().at(-1)).toMatchObject({ wallpaper: 'tide', wallpaper_color: '#336699' })
    expect(radio(t('appearance.solid')).style.backgroundColor, 'the tile keeps the colour').toBe(
      'rgb(51, 102, 153)',
    )
  })
})
