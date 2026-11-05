import { defineStore } from 'pinia'
import { ref } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import type { InterfacePreferences } from '@/shared/api/types'
import { type Locale, setLocale } from '@/shared/i18n'
import { setThemePreference, type ThemePreference, themePreference } from '@/shared/lib/theme'

/**
 * The account's language and colour scheme. They are kept with the account,
 * so every browser it signs in with looks the same; each browser also keeps
 * a copy, so the sign-in page already looks as the account left it.
 */
export const usePreferences = defineStore('preferences', () => {
  const theme = ref<ThemePreference>(themePreference())
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

  return { theme, load, chooseTheme, chooseLanguage }
})
