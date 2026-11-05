// Applies the colour scheme before the first paint, so dark mode does not
// flash. Must stay in sync with src/shared/lib/theme.ts.
try {
  const stored = localStorage.getItem('zendiloos.theme')
  const dark =
    stored === 'dark' ||
    (stored !== 'light' && window.matchMedia('(prefers-color-scheme: dark)').matches)
  document.documentElement.classList.toggle('dark', dark)
} catch {
  // Storage unavailable: stay light.
}
