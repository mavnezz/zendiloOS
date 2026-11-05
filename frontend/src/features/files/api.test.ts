import { afterEach, describe, expect, it, vi } from 'vitest'

import { archiveUrl, previewKind, previewUrl, TEXT_PREVIEW_BYTES, textPreview } from './api'

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('previews', () => {
  it('are offered only for kinds the server shows inline', () => {
    expect(previewKind('Beach.JPG')).toBe('image')
    expect(previewKind('film.webm')).toBe('video')
    expect(previewKind('song.flac')).toBe('audio')
    expect(previewKind('notes.txt')).toBe('text')
    for (const saved of ['page.html', 'logo.svg', 'paper.pdf', 'README', '.png']) {
      expect(previewKind(saved), saved).toBeNull()
    }
  })

  it('download folders as archives', () => {
    expect(archiveUrl(3, 'album')).toBe('/api/v1/files/shares/3/archive?path=album')
  })

  it('ask the server for the file inline', () => {
    expect(previewUrl(3, 'a b/c.png')).toBe(
      '/api/v1/files/shares/3/download?path=a+b%2Fc.png&inline=true',
    )
  })

  it('read only the beginning of a long text', async () => {
    const fetch = vi.fn((_url: string, init: RequestInit) => {
      expect(init.headers).toEqual({ Range: `bytes=0-${TEXT_PREVIEW_BYTES - 1}` })
      return Promise.resolve(
        new Response('hello', {
          status: 206,
          headers: { 'Content-Range': `bytes 0-4/${TEXT_PREVIEW_BYTES + 1}` },
        }),
      )
    })
    vi.stubGlobal('fetch', fetch)

    expect(await textPreview('/x')).toEqual({ text: 'hello', more: true })
  })

  it('know a short text is whole', async () => {
    vi.stubGlobal('fetch', () => Promise.resolve(new Response('short', { status: 200 })))

    expect(await textPreview('/x')).toEqual({ text: 'short', more: false })
  })
})
