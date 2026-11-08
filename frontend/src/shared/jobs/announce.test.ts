import { cleanup, mountApp, on, problem, settle } from '@/shared/testing/server'

import { beforeEach, describe, expect, it } from 'vitest'
import { defineComponent, h } from 'vue'

import { useNotifications } from '@/shared/lib/notifications'
import { job } from '@/shared/testing/fixtures'

import { jobTitle, leftBehind, useJobsStore } from './store'

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

  it('say what a move left where it was, as a warning', async () => {
    const jobs = await store()
    const notifications = useNotifications()
    for (const [id, code, detail] of [
      [5, 'partly_moved', '3 photos/album/new.txt'],
      [6, 'partly_moved', '1 photos/a.jpg'],
      [7, 'copied_not_moved', '1000001 photos/huge'],
    ] as const) {
      jobs.update(job(id, 'files.move'))
      jobs.update(
        job(id, 'files.move', { state: 'failed', error_code: code, error_detail: detail }),
      )
    }
    const bodies = notifications.items.map((n) => n.body ?? '')
    expect(notifications.items.every((n) => n.kind === 'warning')).toBe(true)
    expect(bodies).toContainEqual(
      expect.stringMatching(/3 entries stayed .* photos\/album\/new\.txt/),
    )
    expect(bodies).toContainEqual(expect.stringContaining('photos/a.jpg stayed'))
    expect(bodies).toContainEqual(expect.stringContaining('1000001 entries are too many'))
    expect(bodies.some((body) => body.includes('Failed'))).toBe(false)
    expect(leftBehind('2 a b/c d')).toEqual({ count: 2, example: 'a b/c d' })
    expect(leftBehind(null)).toEqual({ count: 0, example: '' })
  })

  it('name what was not possible', async () => {
    const jobs = await store()
    const why =
      '“photo.jpg” and “Photo.jpg” differ only in case, which the target does not tell apart'
    jobs.update(job(8, 'files.copy'))
    jobs.update(
      job(8, 'files.copy', { state: 'failed', error_code: 'invalid_operation', error_detail: why }),
    )
    expect(useNotifications().items[0]!.body).toContain('“photo.jpg” and “Photo.jpg”')
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
