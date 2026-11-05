import { onScopeDispose, ref, shallowRef } from 'vue'

import { api, unwrap } from '@/shared/api/client'
import { live } from '@/shared/api/live'
import type { Metrics, MetricsHistoryData } from '@/shared/api/types'

/** Samples kept for the charts: six minutes at one sample every two seconds. */
export const HISTORY = 180

export interface MetricsHistory {
  cpu: number[]
  memory: number[]
  received: number[]
  sent: number[]
  read: number[]
  written: number[]
}

const sum = (values: number[]) => values.reduce((total, value) => total + value, 0)
const keep = (series: number[]) => series.slice(-HISTORY)

/** `history` with one more live sample. */
export function append(history: MetricsHistory, sample: Metrics): MetricsHistory {
  return {
    cpu: keep([...history.cpu, sample.cpu_percent]),
    memory: keep([...history.memory, sample.memory.used_bytes]),
    received: keep([
      ...history.received,
      sum(sample.network.map((n) => n.received_bytes_per_second)),
    ]),
    sent: keep([...history.sent, sum(sample.network.map((n) => n.sent_bytes_per_second))]),
    read: keep([...history.read, sum(sample.disks.map((d) => d.read_bytes_per_second))]),
    written: keep([...history.written, sum(sample.disks.map((d) => d.written_bytes_per_second))]),
  }
}

/**
 * The server's last minutes, then the live samples that arrived while they
 * were on their way and are newer.
 */
export function seeded(server: MetricsHistoryData, early: Metrics[]): MetricsHistory {
  let history: MetricsHistory = {
    cpu: keep(server.cpu_percent),
    memory: keep(server.memory_used_bytes),
    received: keep(server.received_bytes_per_second),
    sent: keep(server.sent_bytes_per_second),
    read: keep(server.read_bytes_per_second),
    written: keep(server.written_bytes_per_second),
  }
  const last = server.at.at(-1) ?? 0
  for (const sample of early) {
    if (sample.at > last) history = append(history, sample)
  }
  return history
}

/** Live metrics while the calling component exists, starting with the last minutes. */
export function useLiveMetrics() {
  const latest = shallowRef<Metrics | null>(null)
  const history = ref<MetricsHistory>({
    cpu: [],
    memory: [],
    received: [],
    sent: [],
    read: [],
    written: [],
  })
  let early: Metrics[] | null = []

  const stop = live.subscribe('metrics', (sample) => {
    latest.value = sample
    early?.push(sample)
    history.value = append(history.value, sample)
  })
  onScopeDispose(stop)

  void unwrap(api.GET('/api/v1/system/metrics/history'))
    .then((server) => {
      if (early) history.value = seeded(server, early)
    })
    .catch(() => {
      // The charts fill from live samples alone.
    })
    .finally(() => {
      early = null
    })

  return { latest, history }
}
