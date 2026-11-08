import { callsTo, cleanup, mountApp, on, problem, settle } from '@/shared/testing/server'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { type Component, effectScope, h, provide } from 'vue'

import { i18n } from '@/shared/i18n'
import { useCompactLayout, useMediaQuery } from '@/shared/lib/media'
import {
  applyTheme,
  followSystemTheme,
  setThemePreference,
  themePreference,
} from '@/shared/lib/theme'
import {
  applyWallpaper,
  DEFAULT_WALLPAPER,
  findWallpaper,
  storedWallpaper,
  WALLPAPERS,
} from '@/shared/lib/wallpapers'
import { useWindowSection, WINDOW_SECTION } from '@/shared/lib/window-section'

import { usePreferences } from './store'

/** matchMedia, answering `dark` and telling listeners of a change. */
function media(dark: boolean) {
  const listeners: ((event: { matches: boolean }) => void)[] = []
  const list = {
    matches: dark,
    addEventListener: (_: string, listener: (event: { matches: boolean }) => void) =>
      listeners.push(listener),
    removeEventListener: vi.fn(),
  }
  vi.stubGlobal('matchMedia', () => list)
  return {
    list,
    change: (matches: boolean) => listeners.forEach((listener) => listener({ matches })),
  }
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  media(false)
})

afterEach(() => vi.unstubAllGlobals())

async function store() {
  mountApp({ render: () => h('div') })
  await settle()
  return usePreferences()
}

describe('the preferences', () => {
  it('take what the account chose after signing in', async () => {
    on('GET', '/api/v1/preferences', () => ({
      language: 'de',
      theme: 'dark',
      wallpaper: 'tide',
      widgets: ['network', 'nonsense', 'status'],
      desktop_icons: ['files', 'shares'],
    }))
    const preferences = await store()
    await preferences.load()

    expect(i18n.global.locale.value).toBe('de')
    expect(preferences.theme).toBe('dark')
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    expect(preferences.wallpaper.id).toBe('tide')
    expect(preferences.widgets).toEqual(['network', 'status'])
    expect(preferences.desktopIcons).toEqual(['files', 'shares'])
    i18n.global.locale.value = 'en'
  })

  it('keep what this browser shows when the account has nothing or cannot be asked', async () => {
    on('GET', '/api/v1/preferences', () => problem(500, 'internal'))
    const preferences = await store()
    await preferences.load()
    expect(preferences.widgets).toEqual(['status', 'resources', 'network', 'storage'])

    on('GET', '/api/v1/preferences', () => ({ language: null, theme: null }))
    await preferences.load()
    expect(preferences.theme).toBe('system')
  })

  it('save each choice with the ones before, and keep it when saving fails', async () => {
    let fail = false
    on('PUT', '/api/v1/preferences', ({ body }) => (fail ? problem(500, 'internal') : body))
    const preferences = await store()

    preferences.chooseTheme('light')
    preferences.chooseLanguage('en')
    preferences.chooseWallpaper(WALLPAPERS[5]!)
    preferences.chooseWidgets(['status'])
    fail = true
    preferences.arrangeIcons(['files'])
    await settle()

    expect(callsTo('PUT', '/api/v1/preferences').at(-1)!.body).toEqual({
      language: 'en',
      theme: 'light',
      wallpaper: 'daybreak',
      widgets: ['status'],
      desktop_icons: ['files'],
    })
    expect(preferences.desktopIcons).toEqual(['files'])
    expect(document.documentElement.dataset.wallpaperTone).toBe('light')

    preferences.reset()
    expect(preferences.widgets).toEqual(['status', 'resources', 'network', 'storage'])
    expect(preferences.desktopIcons).toEqual([])
    fail = false
    preferences.chooseTheme('system')
    await settle()
    expect(callsTo('PUT', '/api/v1/preferences').at(-1)!.body).toEqual({
      language: null,
      theme: 'system',
    })
  })
})

describe('the solid background', () => {
  it('comes in the account’s colour, which stays while an image is chosen', async () => {
    on('GET', '/api/v1/preferences', () => ({
      language: null,
      theme: null,
      wallpaper: 'color',
      wallpaper_color: '#dfe2e6',
    }))
    on('PUT', '/api/v1/preferences', ({ body }) => body)
    const preferences = await store()
    await preferences.load()
    expect(preferences.wallpaper.color).toBe('#dfe2e6')
    expect(document.documentElement.dataset.wallpaperTone).toBe('light')

    preferences.chooseWallpaper(findWallpaper('tide'))
    await settle()
    expect(preferences.solidColor).toBe('#dfe2e6')
    expect(callsTo('PUT', '/api/v1/preferences').at(-1)!.body).toMatchObject({
      wallpaper: 'tide',
      wallpaper_color: '#dfe2e6',
    })

    preferences.showWallpaper(findWallpaper('color', '#134e4a'))
    expect(preferences.solidColor).toBe('#134e4a')
    expect(callsTo('PUT', '/api/v1/preferences')).toHaveLength(1)
  })
})

describe('the reminder to star zendiloOS', () => {
  it('is kept with the account: when it was shown, how often, and the answer', async () => {
    on('GET', '/api/v1/preferences', () => ({
      language: null,
      theme: null,
      star_prompt: { times_shown: 1, last_shown_at: 100, answer: 'later' },
    }))
    on('PUT', '/api/v1/preferences', ({ body }) => body)
    const preferences = await store()
    preferences.answerStarPrompt('declined')
    expect(preferences.starPrompt, 'unknown before the account’s choices arrive').toBeNull()
    expect(preferences.loaded).toBe(false)

    await preferences.load()
    expect(preferences.loaded).toBe(true)
    preferences.starPromptShown(200)
    preferences.answerStarPrompt('starred')
    await settle()
    const sent = callsTo('PUT', '/api/v1/preferences').map((call) => call.body)
    expect(sent).toEqual([
      expect.objectContaining({
        star_prompt: { times_shown: 2, last_shown_at: 200, answer: 'later' },
      }),
      expect.objectContaining({
        star_prompt: { times_shown: 2, last_shown_at: 200, answer: 'starred' },
      }),
    ])

    preferences.reset()
    expect([preferences.starPrompt, preferences.loaded]).toEqual([null, false])
  })

  it('counts its first showing', async () => {
    on('GET', '/api/v1/preferences', () => ({ language: null, theme: null }))
    on('PUT', '/api/v1/preferences', ({ body }) => body)
    const preferences = await store()
    await preferences.load()
    preferences.starPromptShown(300)
    expect(preferences.starPrompt).toEqual({ times_shown: 1, last_shown_at: 300, answer: null })
  })
})

describe('the colour scheme', () => {
  it('is remembered in this browser, and follows the system unless chosen', () => {
    const system = media(true)
    expect(themePreference()).toBe('system')
    applyTheme()
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    followSystemTheme()
    system.list.matches = false
    system.change(false)
    expect(document.documentElement.classList.contains('dark')).toBe(false)

    setThemePreference('dark')
    expect(themePreference()).toBe('dark')
    setThemePreference('system')
    expect(localStorage.getItem('zendiloos.theme')).toBeNull()
  })

  it('does without storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    expect(themePreference()).toBe('system')
    expect(() => setThemePreference('dark')).not.toThrow()
    expect(storedWallpaper()).toBe(DEFAULT_WALLPAPER)
    expect(() => applyWallpaper(DEFAULT_WALLPAPER)).not.toThrow()
    vi.restoreAllMocks()
  })
})

describe('backgrounds', () => {
  it('are found by id, the first one otherwise, and remembered', () => {
    expect(findWallpaper('nebula').name).toBe('Nebula')
    expect(findWallpaper('gone')).toBe(DEFAULT_WALLPAPER)
    expect(findWallpaper(null)).toBe(DEFAULT_WALLPAPER)
    applyWallpaper(findWallpaper('abyss'))
    expect(storedWallpaper().id).toBe('abyss')
    expect(document.documentElement.style.getPropertyValue('--wallpaper-color')).toBe('#060d1c')
  })
})

describe('the layout', () => {
  it('follows a media query until its scope ends', () => {
    const query = media(true)
    const scope = effectScope()
    const compact = scope.run(() => useCompactLayout())!
    const wide = scope.run(() => useMediaQuery('(min-width: 1200px)'))!
    expect(compact.value).toBe(true)
    query.change(false)
    expect(compact.value).toBe(false)
    expect(wide.value).toBe(false)
    scope.stop()
    expect(query.list.removeEventListener).toHaveBeenCalledTimes(2)
  })
})

describe('the page of a window', () => {
  it('is the window’s, or the component’s own outside one', async () => {
    const pages = () => ['one', 'two']
    let own: ReturnType<typeof useWindowSection> | undefined
    mountApp({ setup: () => ((own = useWindowSection(pages)), () => h('div')) })
    expect(own!.value).toBe('one')
    own!.value = 'two'
    expect(own!.value).toBe('two')
    own!.value = 'unknown'
    expect(own!.value).toBe('one')

    const navigate = vi.fn()
    let inWindow: ReturnType<typeof useWindowSection> | undefined
    const Inner: Component = { setup: () => ((inWindow = useWindowSection(pages)), () => h('div')) }
    mountApp({
      setup: () => {
        provide(WINDOW_SECTION, { section: { value: 'two' } as never, navigate })
        return () => h(Inner)
      },
    })
    await settle()
    expect(inWindow!.value).toBe('two')
    inWindow!.value = 'one'
    expect(navigate).toHaveBeenCalledWith('one')
    expect(useWindowSection(() => [])).toBeDefined()
  })
})
