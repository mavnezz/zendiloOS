import { describe, expect, it } from 'vitest'

import de from '@/shared/i18n/de'
import en from '@/shared/i18n/en'

import { APPS, findApp, SETTINGS_APP, SETTINGS_SECTIONS } from './apps'

describe('the apps', () => {
  it('each load their component', async () => {
    for (const app of APPS) {
      const loader = (app.component as { __asyncLoader?: () => Promise<unknown> }).__asyncLoader
      if (loader) expect(await loader(), app.id).toBeTruthy()
    }
  }, 60_000)

  // The desktop, the dock and the window's title name an app by `apps.<id>`.
  it('each have a name in every language', () => {
    for (const app of APPS) {
      for (const [locale, messages] of [
        ['en', en],
        ['de', de],
      ] as const) {
        const names: Record<string, string | undefined> = messages.apps
        expect(names[app.id], `${locale}: apps.${app.id}`).toBeTruthy()
      }
    }
  })

  it('are found by id, the control panel among them', () => {
    expect(findApp(SETTINGS_APP)?.sections).toBe(SETTINGS_SECTIONS)
    expect(findApp('nothing')).toBeUndefined()
  })
})
