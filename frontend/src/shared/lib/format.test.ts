import { beforeEach, describe, expect, it } from 'vitest'

import { i18n } from '@/shared/i18n'

import { formatBytes, formatDuration, formatPercent, formatRate } from './format'

beforeEach(() => {
  i18n.global.locale.value = 'en'
})

describe('formatting', () => {
  it('uses binary units with sensible precision', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1,023 B')
    expect(formatBytes(1536)).toBe('1.5 KiB')
    expect(formatBytes(8 * 1024 ** 3)).toBe('8 GiB')
    expect(formatBytes(123.4 * 1024 ** 2)).toBe('123 MiB')
    expect(formatRate(2 * 1024 * 1024)).toBe('2 MiB/s')
  })

  it('follows the interface language', () => {
    i18n.global.locale.value = 'de'
    expect(formatBytes(1536)).toBe('1,5 KiB')
    expect(formatPercent(4.25)).toBe('4,3 %')
  })

  it('shows durations in their two largest units', () => {
    expect(formatDuration(59)).toBe('0 min')
    expect(formatDuration(3 * 3600 + 12 * 60)).toBe('3 h 12 min')
    expect(formatDuration(2 * 86_400 + 5 * 3600)).toBe('2 d 5 h')
  })
})
