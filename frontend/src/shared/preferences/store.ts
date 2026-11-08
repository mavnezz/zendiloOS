import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type {
  InterfacePreferences,
  InterfaceStarPrompt,
  StarPromptAnswer,
} from '@/shared/api/types'
import { type Locale, setLocale } from '@/shared/i18n'
import { setThemePreference, type ThemePreference, themePreference } from '@/shared/lib/theme'
import {
  applyWallpaper,
  DEFAULT_SOLID_COLOR,
  findWallpaper,
  SOLID_WALLPAPER,
  storedWallpaper,
  type Wallpaper,
} from '@/shared/lib/wallpapers'

/** The widgets beside the desktop, by id. */
export const WIDGETS = ['status', 'resources', 'network', 'storage'] as const
export type WidgetId = (typeof WIDGETS)[number]
const DEFAULT_WIDGETS: WidgetId[] = ['status', 'resources', 'network', 'storage']

/**
 * The account's language, colour scheme and desktop: background, widgets
 * and the order of the app icons; and what it answered the reminder to
 * star zendiloOS. They are kept with the account, so every browser it
 * signs in with looks the same; each browser also keeps a copy of
 * language, scheme and background, so the sign-in page already looks as
 * the account left it.
 */
export const usePreferences = defineStore('preferences', () => {
  const theme = ref<ThemePreference>(themePreference())
  const wallpaper = ref<Wallpaper>(storedWallpaper())
  /** The colour of the background of one colour, also while another one is chosen. */
  const solidColor = ref(
    wallpaper.value.id === SOLID_WALLPAPER ? wallpaper.value.color : DEFAULT_SOLID_COLOR,
  )
  const widgets = ref<WidgetId[]>(DEFAULT_WIDGETS)
  /** The account's order of desktop icons; apps not in it go at the end. */
  const desktopIcons = ref<string[]>([])
  /** `null` while the reminder to star zendiloOS was never shown. */
  const starPrompt = ref<InterfaceStarPrompt | null>(null)
  /** Whether the account's choices arrived; until then nothing is known of them. */
  const loaded = ref(false)
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
    if (saved.wallpaper_color) solidColor.value = saved.wallpaper_color
    if (saved.wallpaper) showWallpaper(findWallpaper(saved.wallpaper, saved.wallpaper_color))
    if (saved.widgets) {
      widgets.value = saved.widgets.filter((id): id is WidgetId =>
        (WIDGETS as readonly string[]).includes(id),
      )
    }
    if (saved.desktop_icons) desktopIcons.value = saved.desktop_icons
    starPrompt.value = saved.star_prompt ?? null
    loaded.value = true
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

  /** Shows `next` without keeping it, e.g. while a colour is being picked. */
  function showWallpaper(next: Wallpaper): void {
    wallpaper.value = next
    if (next.id === SOLID_WALLPAPER) solidColor.value = next.color
    applyWallpaper(next)
  }

  function chooseWallpaper(next: Wallpaper): void {
    showWallpaper(next)
    void save(
      next.id === SOLID_WALLPAPER
        ? { wallpaper: next.id, wallpaper_color: next.color }
        : { wallpaper: next.id },
    )
  }

  function chooseWidgets(next: WidgetId[]): void {
    widgets.value = next
    void save({ widgets: next })
  }

  function arrangeIcons(next: string[]): void {
    desktopIcons.value = next
    void save({ desktop_icons: next })
  }

  /** The reminder to star zendiloOS appeared now (Unix seconds). */
  function starPromptShown(now: number): void {
    starPrompt.value = {
      times_shown: (starPrompt.value?.times_shown ?? 0) + 1,
      last_shown_at: now,
      answer: starPrompt.value?.answer ?? null,
    }
    void save({ star_prompt: starPrompt.value })
  }

  function answerStarPrompt(answer: StarPromptAnswer): void {
    if (!starPrompt.value) return
    starPrompt.value = { ...starPrompt.value, answer }
    void save({ star_prompt: starPrompt.value })
  }

  /**
   * After signing out: another account may sign in next, and nothing of
   * this one's is kept to be saved for it. Language, scheme and background
   * stay as this browser shows them.
   */
  function reset(): void {
    saved = { language: null, theme: null }
    widgets.value = [...DEFAULT_WIDGETS]
    desktopIcons.value = []
    starPrompt.value = null
    loaded.value = false
  }

  return {
    theme,
    wallpaper,
    solidColor,
    widgets,
    desktopIcons,
    starPrompt,
    loaded,
    load,
    chooseTheme,
    chooseLanguage,
    showWallpaper,
    chooseWallpaper,
    chooseWidgets,
    arrangeIcons,
    starPromptShown,
    answerStarPrompt,
    reset,
  }
})
