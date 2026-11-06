import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ServerNotification } from '@/shared/api/types'
import { useNotifications } from '@/shared/lib/notifications'

import { useServerNotices } from './store'

const calls = vi.hoisted(() => ({
  list: { notifications: [] as unknown[], read_up_to: 0 },
  posted: [] as { path: string; body: unknown }[],
  live: null as ((notification: unknown) => void) | null,
}))

vi.mock('@/shared/api/client', () => ({
  api: {
    GET: () => Promise.resolve(calls.list),
    POST: (path: string, options: { body: unknown }) => {
      calls.posted.push({ path, body: options.body })
      return Promise.resolve(undefined)
    },
  },
  unwrap: (value: unknown) => Promise.resolve(value),
}))

vi.mock('@/shared/api/live', () => ({
  live: {
    subscribe: (_topic: string, listener: (notification: unknown) => void) => {
      calls.live = listener
      return () => (calls.live = null)
    },
  },
}))

function notice(id: number): ServerNotification {
  return { id, at: id, level: 'warning', code: 'pool_trouble', params: { pool: 'tank' } }
}

beforeEach(() => {
  setActivePinia(createPinia())
  calls.posted = []
  calls.live = null
})

describe('server notifications', () => {
  it('shows what the account has not read yet as unread, once', async () => {
    calls.list = { notifications: [notice(3), notice(2), notice(1)], read_up_to: 2 }
    const notices = useServerNotices()

    const started = notices.start()
    // Arrives live while the list is on its way, and is in the list too.
    calls.live?.(notice(3))
    await started

    const items = useNotifications().items
    expect(items).toHaveLength(3)
    expect(items.map((item) => item.read)).toEqual([false, true, true])
    expect(useNotifications().toasts).toHaveLength(1)
  })

  it('marks read and clears up to the newest shown, for every browser', async () => {
    calls.list = { notifications: [notice(5)], read_up_to: 0 }
    const notices = useServerNotices()
    await notices.start()
    calls.live?.(notice(6))

    await notices.markRead()
    await notices.clear()

    expect(calls.posted).toEqual([
      { path: '/api/v1/notifications/read', body: { up_to: 6 } },
      { path: '/api/v1/notifications/clear', body: { up_to: 6 } },
    ])
  })

  it('sends nothing while nothing was shown', async () => {
    calls.list = { notifications: [], read_up_to: 0 }
    const notices = useServerNotices()
    await notices.start()

    await notices.markRead()

    expect(calls.posted).toEqual([])
  })
})

describe('the wording of update notices', () => {
  const notice = (code: string, params: Record<string, string>): ServerNotification => ({
    id: 1,
    at: 0,
    level: 'info',
    code,
    params,
  })

  it('names a release only when there is one, and warns before a restart without ZFS', async () => {
    const { wording } = await import('./store')
    const { i18n } = await import('@/shared/i18n')
    i18n.global.locale.value = 'en'
    expect(
      wording(notice('updates_available', { version: '0.2.0', packages: '3', security: '1' })).body,
    ).toBe('zendiloOS 0.2.0 and 3 Debian packages, 1 of them security updates.')
    expect(wording(notice('updates_available', { packages: '3', security: '1' })).body).toBe(
      '3 Debian packages, 1 of them security updates.',
    )
    expect(
      wording(notice('updates_installed', { restart: 'true', zfs_ready: 'true' })).body,
    ).toContain('restart')
    expect(
      wording(notice('updates_installed', { restart: 'true', zfs_ready: 'false' })).body,
    ).toContain('Do not restart')
  })
})
