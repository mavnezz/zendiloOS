import { afterEach, describe, expect, it, vi } from 'vitest'

import { passphraseHeader, waitForServer } from './passphrase'

afterEach(() => vi.unstubAllGlobals())

describe('a passphrase in a header', () => {
  it('is base64 of its UTF-8', () => {
    expect(passphraseHeader('correct horse')).toBe('Y29ycmVjdCBob3JzZQ==')
    expect(atob(passphraseHeader('Grüße'))).toBe('GrÃ¼Ã\u009fe')
  })
})

describe('waiting for the server', () => {
  it('waits until it answers, or gives up', async () => {
    let calls = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(() => {
        calls += 1
        return calls < 3 ? Promise.reject(new Error('down')) : Promise.resolve(new Response('{}'))
      }),
    )
    expect(await waitForServer(1, 1)).toBe(true)
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response('', { status: 502 }))),
    )
    expect(await waitForServer(0.01, 1)).toBe(false)
  })
})
