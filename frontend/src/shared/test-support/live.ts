/**
 * A stand-in for the live connection in component tests: what is
 * subscribed, and samples pushed by the test. Tests route to it with
 *
 *     vi.mock('@/shared/api/live', async () => ({ live: (await import('@/shared/test-support/live')).live }))
 */

import type { Metrics } from '@/shared/api/types'

type Listener = (data: never) => void

const listeners = new Map<string, Set<Listener>>()
const reconnects = new Set<() => void>()

export const live = {
  connected: false,
  onSessionEnded: () => undefined as void,
  connect(): void {
    live.connected = true
  },
  disconnect(): void {
    live.connected = false
  },
  subscribe(topic: string, listener: Listener): () => void {
    const set = listeners.get(topic) ?? new Set<Listener>()
    set.add(listener)
    listeners.set(topic, set)
    return () => set.delete(listener)
  },
  onReconnect(listener: () => void): () => void {
    reconnects.add(listener)
    return () => reconnects.delete(listener)
  },
}

/** Hands `data` to everyone listening to `topic`. */
export function push(topic: string, data: unknown): void {
  for (const listener of listeners.get(topic) ?? []) (listener as (data: unknown) => void)(data)
}

/** Whether anyone listens to `topic`. */
export function listened(topic: string): boolean {
  return (listeners.get(topic)?.size ?? 0) > 0
}

/** A live sample of the NAS. */
export function sample(at: number, extra: Partial<Metrics> = {}): Metrics {
  return {
    at,
    cpu_percent: 23,
    load_average: [0.5, 0.4, 0.3],
    memory: { total_bytes: 8e9, used_bytes: 2e9, available_bytes: 6e9 },
    network: [{ name: 'eth0', received_bytes_per_second: 1e6, sent_bytes_per_second: 2e5 }],
    disks: [{ name: 'sda', read_bytes_per_second: 3e6, written_bytes_per_second: 1e6 }],
    temperatures: [
      { label: 'Package id 0', celsius: 71 },
      { label: 'nvme', celsius: 85 },
      { label: 'acpitz', celsius: 40 },
    ],
    uptime_seconds: 90_000,
    ...extra,
  }
}
