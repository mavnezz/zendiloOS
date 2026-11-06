import { describe, expect, it } from 'vitest'

import type { UsageMoment } from '@/shared/api/types'

import { containersIn, imageParts, usageSeries } from './usage'

const point = (container: string, cpu: number) => ({
  container,
  cpu_percent: cpu,
  memory_bytes: cpu * 1000,
  received_per_second: 1,
  sent_per_second: 2,
})

const MOMENTS: UsageMoment[] = [
  { at: 0, points: [point('db', 1)] },
  { at: 20, points: [point('db', 3), point('web', 9)] },
]

describe('the containers use', () => {
  it('becomes one series per container, the busiest first', () => {
    expect(containersIn(MOMENTS)).toEqual(['db', 'web'])
    const series = usageSeries(MOMENTS, 'cpu')
    expect(series.map((s) => [s.label, s.values])).toEqual([
      ['web', [0, 9]],
      ['db', [1, 3]],
    ])
    expect(usageSeries(MOMENTS, 'sent', 1)).toHaveLength(1)
  })

  it('names images by name and tag', () => {
    expect(imageParts('postgres:17')).toEqual({ name: 'postgres', tag: '17' })
    expect(imageParts('ghcr.io/owner/app')).toEqual({ name: 'ghcr.io/owner/app', tag: 'latest' })
    expect(imageParts('localhost:5000/app:1.2@sha256:abc')).toEqual({
      name: 'localhost:5000/app',
      tag: '1.2',
    })
  })
})
