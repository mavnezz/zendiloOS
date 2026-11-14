import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ServerNotification } from '@/shared/api/types'
import { useNotifications } from '@/shared/lib/notifications'

import { useServerNotices } from './store'

const calls = vi.hoisted(() => ({
  list: { notifications: [] as unknown[], read_up_to: 0 },
  posted: [] as { path: string; body: unknown }[],
  live: null as ((notification: unknown) => void) | null,
  reconnected: null as (() => void) | null,
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
    onReconnect: (listener: () => void) => {
      calls.reconnected = listener
      return () => (calls.reconnected = null)
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

  it('shows what came while the connection was down once it is back', async () => {
    calls.list = { notifications: [notice(1)], read_up_to: 0 }
    const notices = useServerNotices()
    await notices.start()

    calls.list = { notifications: [notice(3), notice(2), notice(1)], read_up_to: 0 }
    calls.reconnected?.()
    await vi.waitFor(() => expect(useNotifications().items).toHaveLength(3))

    notices.stop()
    expect(calls.reconnected).toBeNull()
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

describe('the wording of UPS notices', () => {
  const notice = (code: string, params: Record<string, string>): ServerNotification => ({
    id: 1,
    at: 0,
    level: 'warning',
    code,
    params,
  })

  it('says by which rule the NAS shuts down, what the battery holds, and why it went down', async () => {
    const { wording } = await import('./store')
    const { i18n } = await import('@/shared/i18n')
    i18n.global.locale.value = 'en'
    expect(wording(notice('ups_on_battery', { when: 'low_battery' }))).toEqual({
      title: 'Power cut: the UPS runs on battery',
      body: 'Unless the power returns in time, zendiloOS shuts down when the UPS reports its battery as low.',
    })
    expect(
      wording(
        notice('ups_on_battery', {
          when: 'charge_below',
          percent: '40',
          charge: '87',
          runtime: '25',
        }),
      ).body,
    ).toBe(
      'Unless the power returns in time, zendiloOS shuts down when the charge falls below 40 %. The battery is at 87 % and lasts about 25 min.',
    )
    expect(
      wording(notice('ups_on_battery', { when: 'after_minutes', minutes: '10', charge: '87' }))
        .body,
    ).toBe(
      'Unless the power returns in time, zendiloOS shuts down after 10 min on battery. The battery is at 87 %.',
    )
    expect(wording(notice('ups_shutdown', { why: 'battery' })).body).toBe(
      "The UPS's battery is almost empty.",
    )
    expect(wording(notice('ups_shutdown', { why: 'time' })).body).toContain('Control panel → UPS')
    for (const code of ['ups_on_mains', 'ups_unreachable', 'ups_replace_battery'])
      expect(wording(notice(code, {})).body).toBeTruthy()
  })
})

describe('the wording of firewall notices', () => {
  const notice = (params: Record<string, string>): ServerNotification => ({
    id: 1,
    at: 0,
    level: 'warning',
    code: 'firewall_blocked',
    params,
  })

  it('names the service or project the rules block', async () => {
    const { wording } = await import('./store')
    const { i18n } = await import('@/shared/i18n')
    i18n.global.locale.value = 'en'
    expect(wording(notice({ service: 'smb' })).title).toBe('The firewall blocks SMB')
    expect(wording(notice({ service: 'web_interface' })).title).toBe(
      'The firewall blocks the web interface',
    )
    expect(wording(notice({ service: 'project:media', project: 'media' })).title).toBe(
      'The firewall blocks the Docker project media',
    )
    expect(wording(notice({ service: 'odd' })).title).toBe('The firewall blocks odd')
    i18n.global.locale.value = 'de'
    expect(wording(notice({ service: 'discovery' })).title).toBe(
      'Die Firewall sperrt die Netzwerkerkennung',
    )
    i18n.global.locale.value = 'en'
  })
})

describe('the wording of disk notices', () => {
  const notice = (params: Record<string, string>): ServerNotification => ({
    id: 1,
    at: 0,
    level: 'error',
    code: 'disk_health',
    params,
  })

  it('names a disk with its slot where the enclosure places it', async () => {
    const { wording } = await import('./store')
    const { i18n } = await import('@/shared/i18n')
    i18n.global.locale.value = 'en'
    expect(wording(notice({ disk: 'sda', model: 'WD RED', health: 'failing' })).title).toBe(
      'Disk sda reports problems',
    )
    const placed = notice({ disk: 'sda', health: 'failing', slot: 'Bay 1', group: 'Front bays' })
    expect(wording(placed).title).toBe('Disk sda (Bay 1, Front bays) reports problems')
  })

  it('tells of an eMMC’s wear as such, as the system has to move', async () => {
    const { wording } = await import('./store')
    const { i18n } = await import('@/shared/i18n')
    i18n.global.locale.value = 'en'
    const worn = notice({ disk: 'mmcblk0', model: 'DV4032', health: 'warning', source: 'emmc' })
    expect(wording(worn).body).toContain('the eMMC’s wear is')
    expect(wording(worn).body).toContain('Move the system to another disk')
    expect(wording(notice({ disk: 'sda', health: 'warning' })).body).toContain('SMART health')
  })
})
