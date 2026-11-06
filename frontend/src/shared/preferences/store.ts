import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { InterfacePreferences } from '@/shared/api/types'
import { type Locale, setLocale } from '@/shared/i18n'
import { setThemePreference, type ThemePreference, themePreference } from '@/shared/lib/theme'
import {
  applyWallpaper,
  findWallpaper,
  storedWallpaper,
  type Wallpaper,
} from '@/shared/lib/wallpapers'

/** The widgets beside the desktop, by id. */
export const WIDGETS = ['status', 'resources', 'network', 'storage'] as const
export type WidgetId = (typeof WIDGETS)[number]
const DEFAULT_WIDGETS: WidgetId[] = ['status', 'resources', 'network', 'storage']

/**
 * The account's language, colour scheme and desktop: background, widgets
 * and the order of the app icons. They are kept with the account, so every
 * browser it signs in with looks the same; each browser also keeps a copy
 * of language, scheme and background, so the sign-in page already looks as
 * the account left it.
 */
export const usePreferences = defineStore('preferences', () => {
  const theme = ref<ThemePreference>(themePreference())
  const wallpaper = ref<Wallpaper>(storedWallpaper())
  const widgets = ref<WidgetId[]>(DEFAULT_WIDGETS)
  /** The account's order of desktop icons; apps not in it go at the end. */
  const desktopIcons = ref<string[]>([])
  let saved: InterfacePreferences = { language: null, theme: null }

  function showTheme(next: ThemePreference): void {
    theme.value = next
    setThemePreference(next)
  }

  /** After signing in: what the account chose wins over this browser. */
  async function load(): Promise<void> {
    try {
      saved = await unwrap(api.GET('/api/v1/preferences'))
    } catch {
      return // This browser keeps what it shows.
    }
    if (saved.language) setLocale(saved.language)
    if (saved.theme) showTheme(saved.theme)
    if (saved.wallpaper) {
      wallpaper.value = findWallpaper(saved.wallpaper)
      applyWallpaper(wallpaper.value)
    }
    if (saved.widgets) {
      widgets.value = saved.widgets.filter((id): id is WidgetId =>
        (WIDGETS as readonly string[]).includes(id),
      )
    }
    if (saved.desktop_icons) desktopIcons.value = saved.desktop_icons
  }

  async function save(change: Partial<InterfacePreferences>): Promise<void> {
    saved = { ...saved, ...change }
    try {
      await unwrap(api.PUT('/api/v1/preferences', { body: saved }))
    } catch {
      // The choice still applies in this browser.
    }
  }

  function chooseTheme(next: ThemePreference): void {
    showTheme(next)
    void save({ theme: next })
  }

  function chooseLanguage(next: Locale): void {
    setLocale(next)
    void save({ language: next })
  }

  function chooseWallpaper(next: Wallpaper): void {
    wallpaper.value = next
    applyWallpaper(next)
    void save({ wallpaper: next.id })
  }

  function chooseWidgets(next: WidgetId[]): void {
    widgets.value = next
    void save({ widgets: next })
  }

  function arrangeIcons(next: string[]): void {
    desktopIcons.value = next
    void save({ desktop_icons: next })
  }

  return {
    theme,
    wallpaper,
    widgets,
    desktopIcons,
    load,
    chooseTheme,
    chooseLanguage,
    chooseWallpaper,
    chooseWidgets,
    arrangeIcons,
  }
})
