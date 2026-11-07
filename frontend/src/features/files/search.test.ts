import { answer, callsTo, cleanup } from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'

import { searchFolder } from './api'

const NOTHING = { path: '', entries: [], stopped: null }

beforeEach(() => {
  cleanup()
})

describe('searching a folder on the NAS', () => {
  it('asks the share’s or the drive’s route, with hidden names and a snapshot only when wanted', async () => {
    answer('GET', '/api/v1/files/shares/3/search', NOTHING)
    answer('GET', '/api/v1/files/removable/1A2B-3C4D/search', NOTHING)

    await searchFolder(3, { path: 'a', text: 'beach', hidden: false, snapshot: null })
    const plain = callsTo('GET', '/api/v1/files/shares/3/search')[0]!.query
    expect([
      plain.get('path'),
      plain.get('text'),
      plain.has('hidden'),
      plain.has('snapshot'),
    ]).toEqual(['a', 'beach', false, false])
    await searchFolder(3, {
      path: '',
      text: 'x',
      hidden: true,
      snapshot: 'zdl-2026-11-05_19.40.01',
    })
    const full = callsTo('GET', '/api/v1/files/shares/3/search')[1]!.query
    expect([full.get('hidden'), full.get('snapshot')]).toEqual(['true', 'zdl-2026-11-05_19.40.01'])

    const found = await searchFolder('1A2B-3C4D', {
      path: '',
      text: 'song',
      hidden: false,
      snapshot: null,
    })
    expect(found).toEqual(NOTHING)
    expect(callsTo('GET', '/api/v1/files/removable/1A2B-3C4D/search')[0]!.query.get('text')).toBe(
      'song',
    )
  })
})
