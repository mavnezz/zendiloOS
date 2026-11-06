import { describe, expect, it } from 'vitest'

import type { Metrics, MetricsHistoryData } from '@/shared/api/types'

import { appendDevices, HISTORY, seeded } from './metrics'

function sample(at: number, cpu: number): Metrics {
  return {
    at,
    cpu_percent: cpu,
    load_average: [0, 0, 0],
    memory: { total_bytes: 8, used_bytes: 2, available_bytes: 6 },
    network: [{ name: 'eth0', received_bytes_per_second: 10, sent_bytes_per_second: 1 }],
    disks: [],
    temperatures: [],
    uptime_seconds: 1,
  }
}

function server(at: number[], cpu: number[]): MetricsHistoryData {
  const zeros = at.map(() => 0)
  return {
    at,
    cpu_percent: cpu,
    memory_used_bytes: zeros,
    received_bytes_per_second: zeros,
    sent_bytes_per_second: zeros,
    read_bytes_per_second: zeros,
    written_bytes_per_second: zeros,
  }
}

describe('the metrics history', () => {
  it('starts with the server minutes and adds only newer live samples', () => {
    const history = seeded(server([96, 98, 100], [1, 2, 3]), [sample(100, 3), sample(102, 4)])

    expect(history.cpu).toEqual([1, 2, 3, 4])
    expect(history.received).toEqual([0, 0, 0, 10])
  })

  it('keeps no more than the charts show', () => {
    const at = Array.from({ length: HISTORY + 20 }, (_, i) => i)

    expect(seeded(server(at, at), []).cpu).toHaveLength(HISTORY)
  })
})

describe('the history per device', () => {
  it('grows per name and lets devices that are gone drop out', () => {
    let devices = appendDevices({}, [
      { name: 'eth0', values: [1, 2] },
      { name: 'wg0', values: [3, 4] },
    ])
    devices = appendDevices(devices, [{ name: 'eth0', values: [5, 6] }])
    expect(devices).toEqual({
      eth0: [
        [1, 5],
        [2, 6],
      ],
    })
  })
})
