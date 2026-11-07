import { describe, expect, it } from 'vitest'

import { APPS, findApp, SETTINGS_APP, SETTINGS_SECTIONS } from './apps'

describe('the apps', () => {
  it('each load their component', async () => {
    for (const app of APPS) {
      const loader = (app.component as { __asyncLoader?: () => Promise<unknown> }).__asyncLoader
      if (loader) expect(await loader(), app.id).toBeTruthy()
    }
  }, 60_000)

  it('are found by id, the control panel among them', () => {
    expect(findApp(SETTINGS_APP)?.sections).toBe(SETTINGS_SECTIONS)
    expect(findApp('nothing')).toBeUndefined()
  })
})
