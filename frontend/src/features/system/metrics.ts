import { onScopeDispose, ref, shallowRef } from 'vue'

import { live } from '@/shared/api/live'
import type { Metrics } from '@/shared/api/types'

/** Samples kept for the charts: two minutes at one sample every two seconds. */
export const HISTORY = 60

export interface MetricsHistory {
  cpu: number[]
  memory: number[]
  received: number[]
  sent: number[]
  read: number[]
  written: number[]
}

const sum = (values: number[]) => values.reduce((total, value) => total + value, 0)

function push(series: number[], value: number): number[] {
  return [...series, value].slice(-HISTORY)
}

/** Live metrics while the calling component exists. */
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

  const stop = live.subscribe('metrics', (sample) => {
    latest.value = sample
    const h = history.value
    history.value = {
      cpu: push(h.cpu, sample.cpu_percent),
      memory: push(h.memory, sample.memory.used_bytes),
      received: push(h.received, sum(sample.network.map((n) => n.received_bytes_per_second))),
      sent: push(h.sent, sum(sample.network.map((n) => n.sent_bytes_per_second))),
      read: push(h.read, sum(sample.disks.map((d) => d.read_bytes_per_second))),
      written: push(h.written, sum(sample.disks.map((d) => d.written_bytes_per_second))),
    }
  })
  onScopeDispose(stop)

  return { latest, history }
}
