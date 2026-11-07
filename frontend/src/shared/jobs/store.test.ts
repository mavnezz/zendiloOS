import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Job } from '@/shared/api/types'
import { useNotifications } from '@/shared/lib/notifications'

import { useJobsStore } from './store'

const calls = vi.hoisted(() => ({
  list: [] as unknown[],
  live: null as ((job: unknown) => void) | null,
  reconnected: null as (() => void) | null,
}))

vi.mock('@/shared/api/client', () => ({
  api: { GET: () => Promise.resolve(calls.list) },
  unwrap: (value: unknown) => Promise.resolve(value),
}))

vi.mock('@/shared/api/live', () => ({
  live: {
    subscribe: (_topic: string, listener: (job: unknown) => void) => {
      calls.live = listener
      return () => (calls.live = null)
    },
    onReconnect: (listener: () => void) => {
      calls.reconnected = listener
      return () => (calls.reconnected = null)
    },
  },
}))

function job(id: number, state: Job['state']): Job {
  return {
    id,
    kind: 'sharing.apply_access',
    target: 'photos',
    state,
    progress: null,
    step: null,
    error_code: null,
    error_detail: null,
    actor: 'admin',
    created_at: id,
    updated_at: id,
    finished_at: state === 'running' ? null : id,
  }
}

beforeEach(() => {
  setActivePinia(createPinia())
  calls.live = null
  calls.reconnected = null
})

describe('jobs', () => {
  it('learns after a reconnect what ended while the connection was down', async () => {
    calls.list = [job(1, 'running')]
    const jobs = useJobsStore()
    await jobs.start()
    expect(jobs.running).toHaveLength(1)

    calls.list = [job(2, 'running'), job(1, 'succeeded')]
    calls.reconnected?.()
    await vi.waitFor(() => expect(jobs.running.map((j) => j.id)).toEqual([2]))
    expect(useNotifications().items.map((item) => item.kind)).toEqual(['success'])

    jobs.stop()
    expect(calls.reconnected).toBeNull()
    expect(calls.live).toBeNull()
  })
})
