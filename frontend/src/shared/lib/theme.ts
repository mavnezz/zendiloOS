// Light or dark mode. public/theme-init.js applies the same rule before the
// first paint.

export type ThemePreference = 'light' | 'dark' | 'system'

const STORAGE_KEY = 'zendiloos.theme'
const prefersDark = () => window.matchMedia('(prefers-color-scheme: dark)')

export function themePreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY)
    return stored === 'light' || stored === 'dark' ? stored : 'system'
  } catch {
    return 'system'
  }
}

export function applyTheme(preference: ThemePreference = themePreference()): void {
  const dark = preference === 'dark' || (preference === 'system' && prefersDark().matches)
  document.documentElement.classList.toggle('dark', dark)
}

/** Follows the operating system's setting while the preference is `system`. */
export function followSystemTheme(): void {
  prefersDark().addEventListener('change', () => applyTheme())
}
