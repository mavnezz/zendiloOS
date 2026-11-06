import { describe, expect, it } from 'vitest'

import { divisions, monotonePath, niceMax } from './scale'

describe('chart scales', () => {
  it('round up to numbers that read well', () => {
    expect(niceMax(0)).toBe(1)
    expect(niceMax(73)).toBe(100)
    expect(niceMax(18)).toBe(20)
    expect(niceMax(2.2)).toBe(2.5)
    expect(niceMax(3_000, 1024)).toBe(5 * 1024)
    // 1.5 MiB/s: a scale of 2 MiB/s.
    expect(niceMax(1.5 * 1024 * 1024, 1024)).toBe(2 * 1024 * 1024)
  })

  it('divide the scale into round steps', () => {
    expect(divisions(100)).toBe(5)
    expect(divisions(5 * 1024 * 1024)).toBe(5)
    expect(divisions(2.5)).toBe(5)
    expect(divisions(20)).toBe(4)
  })

  it('curves never overshoot what was measured', () => {
    const path = monotonePath([
      { x: 0, y: 10 },
      { x: 10, y: 10 },
      { x: 20, y: 0 },
      { x: 30, y: 0 },
    ])
    const ys = [...path.matchAll(/(-?\d+\.\d),(-?\d+\.\d)/g)].map((m) => Number(m[2]))
    expect(Math.max(...ys)).toBeLessThanOrEqual(10)
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(0)
    expect(monotonePath([])).toBe('')
  })
})
