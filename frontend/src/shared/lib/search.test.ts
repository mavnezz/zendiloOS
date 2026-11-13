import { describe, expect, it } from 'vitest'

import { matchesWords } from './search'

describe('searching by words', () => {
  it('wants every word, in any order and case', () => {
    expect(matchesWords('Time & language Clock NTP', 'ntp time')).toBe(true)
    expect(matchesWords('Time & language Clock NTP', 'ntp disks')).toBe(false)
    expect(matchesWords('Zeit & Sprache', 'SPRACH')).toBe(true)
  })

  it('matches everything while nothing is asked', () => {
    expect(matchesWords('Network', '')).toBe(true)
    expect(matchesWords('Network', '   ')).toBe(true)
  })
})
