import { cleanup, mountApp, on, problem, settle } from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'
import { defineComponent, h } from 'vue'

import { useNotifications } from '@/shared/lib/notifications'
import { job } from '@/shared/testing/fixtures'

import { jobTitle, useJobsStore } from './store'

async function store() {
  mountApp(defineComponent({ render: () => h('div') }))
  await settle()
  return useJobsStore()
}

beforeEach(() => cleanup())

describe('jobs as people see them', () => {
  it('are named by their kind and target, or by their kind as it is', () => {
    expect(jobTitle(job(1, 'storage.create_pool', { target: 'tank' }))).toMatch(/: tank$/)
    expect(jobTitle(job(1, 'something.new'))).toBe('something.new')
  })

  it('say why they failed, in words where the reason is known', async () => {
    const jobs = await store()
    const notifications = useNotifications()
    for (const [id, code] of [
      [1, 'agent_failed'],
      [2, 'unheard_of'],
      [3, null],
    ] as const) {
      jobs.update(job(id, 'files.copy'))
      jobs.update(job(id, 'files.copy', { state: 'failed', error_code: code }))
    }
    const bodies = notifications.items.map((n) => n.body ?? '')
    expect(notifications.items.every((n) => n.kind === 'error')).toBe(true)
    expect(bodies.some((body) => body.includes('unheard_of'))).toBe(true)
    expect(new Set(bodies).size).toBe(3)
  })

  it('are waited for until they end', async () => {
    const jobs = await store()
    jobs.update(job(4, 'files.move'))
    const ended = jobs.settled(4)
    jobs.update(job(4, 'files.move', { state: 'succeeded' }))
    // Ended already: answered at once.
    expect((await jobs.settled(4)).state).toBe('succeeded')
    void ended
  })

  it('start without the list when the server cannot give it, and only once', async () => {
    on('GET', '/api/v1/jobs', () => problem(500, 'internal'))
    const jobs = await store()
    await jobs.start()
    await jobs.start()
    expect(jobs.list).toEqual([])
    jobs.stop()
  })
})
