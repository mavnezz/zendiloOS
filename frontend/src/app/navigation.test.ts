import { describe, expect, it } from 'vitest'

import { type AuthState, type Page, redirectFor } from './navigation'

describe('redirectFor', () => {
  const pages: Page[] = ['setup', 'login', 'password', 'desktop']
  const cases: [AuthState, Page][] = [
    [{ signedIn: false, setupRequired: true, passwordChangeRequired: false }, 'setup'],
    [{ signedIn: false, setupRequired: false, passwordChangeRequired: false }, 'login'],
    [{ signedIn: true, setupRequired: false, passwordChangeRequired: true }, 'password'],
    [{ signedIn: true, setupRequired: false, passwordChangeRequired: false }, 'desktop'],
  ]

  it.each(cases)('sends every navigation in state %j to the %s page', (state, page) => {
    for (const target of pages) {
      expect(redirectFor(target, state)).toBe(target === page ? null : page)
    }
  })
})
