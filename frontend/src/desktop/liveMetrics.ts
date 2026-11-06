import { onScopeDispose, shallowRef } from 'vue'

import { live } from '@/shared/api/live'
import type { Metrics } from '@/shared/api/types'

/** The newest live sample while the caller exists: for the bar and the widgets. */
export function useLatestMetrics() {
  const latest = shallowRef<Metrics | null>(null)
  onScopeDispose(live.subscribe('metrics', (sample) => (latest.value = sample)))
  return latest
}

export function memoryRatio(sample: Metrics): number {
  return sample.memory.total_bytes > 0 ? sample.memory.used_bytes / sample.memory.total_bytes : 0
}

export function networkRates(sample: Metrics): { sent: number; received: number } {
  return sample.network.reduce(
    (total, n) => ({
      sent: total.sent + n.sent_bytes_per_second,
      received: total.received + n.received_bytes_per_second,
    }),
    { sent: 0, received: 0 },
  )
}
