import { describe, expect, it } from 'vitest'

import { LOG_COLORS, logColors, matching, parseProjectLog } from './projectLog'

const LOG = [
  'teslamate-grafana-1  | logger=cleanup msg="Completed cleanup jobs"',
  'teslamate-1          | 2026-11-07 [info] System started',
  '  continued on the next line',
  '',
  'teslamate-db-1       |',
].join('\n')

describe('a project’s log', () => {
  it('puts each line under the container before its bar, a wrapped one under the one before', () => {
    expect(parseProjectLog(LOG)).toEqual([
      { container: 'teslamate-grafana-1', text: 'logger=cleanup msg="Completed cleanup jobs"' },
      { container: 'teslamate-1', text: '2026-11-07 [info] System started' },
      { container: 'teslamate-1', text: '  continued on the next line' },
      { container: 'teslamate-db-1', text: '' },
    ])
    expect(parseProjectLog('Compose says something\n')).toEqual([
      { container: null, text: 'Compose says something' },
    ])
  })

  it('gives each container a colour of its own, the project’s first', () => {
    const colors = logColors(['teslamate-db-1', 'teslamate-1'], parseProjectLog(LOG))
    expect(colors.get('teslamate-1')).toBe(LOG_COLORS[0])
    expect(colors.get('teslamate-db-1')).toBe(LOG_COLORS[1])
    expect(colors.get('teslamate-grafana-1')).toBe(LOG_COLORS[2])
    const many = logColors(
      Array.from({ length: LOG_COLORS.length + 1 }, (_, n) => `c${String(n).padStart(2, '0')}`),
      [],
    )
    expect(many.get('c08')).toBe(LOG_COLORS[0])
  })

  it('finds lines by what they say or by their container, case aside', () => {
    const lines = parseProjectLog(LOG)
    expect(matching(lines, '  ')).toBe(lines)
    expect(matching(lines, 'CLEANUP').map((line) => line.container)).toEqual([
      'teslamate-grafana-1',
    ])
    expect(matching(lines, 'db-1')).toHaveLength(1)
  })
})
