import { beforeEach, describe, expect, it, vi } from 'vitest'

import { mountApp, page, settle } from '@/shared/test-support/mount'
import { called, on, problem, reset, type Call } from '@/shared/test-support/server'
import type { NotificationChannel, NotificationRoute, RouteChoice } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import RoutesTable from './RoutesTable.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const channel = (id: number, name: string): NotificationChannel => ({
  id,
  name,
  target: { kind: 'webhook', url: `https://${name}.example.org`, format: 'json' },
  recipients: [],
  language: 'en',
  last_delivery_at: null,
  last_error: null,
})
const channels = [channel(1, 'Mail'), channel(2, 'Chat')]

let routes: NotificationRoute[]

beforeEach(() => {
  reset()
  routes = [
    { kind: 'pool_trouble', group: 'storage', level: 'warning', in_app: true, channels: [1] },
    { kind: 'scrub_finished', group: 'storage', level: 'info', in_app: true, channels: [] },
    { kind: 'snapshot_failed', group: 'folders', level: 'warning', in_app: false, channels: [2] },
    { kind: 'weather', group: 'system', level: 'info', in_app: true, channels: [] },
  ]
  on('GET', '/api/v1/notification-routes', () => routes)
  on('PUT', '/api/v1/notification-routes', (call: Call) => {
    const [choice] = (call.body as { kinds: RouteChoice[] }).kinds
    routes = routes.map((route) => (route.kind === choice!.kind ? { ...route, ...choice } : route))
    return routes
  })
})

function cell(kind: string, to: string): HTMLInputElement {
  const label = t('channels.routes.cell', { kind: t(`channels.routes.kinds.${kind}`), to })
  const found = document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)
  if (!found) throw new Error(`no cell ${label}`)
  return found
}

async function tick(box: HTMLInputElement): Promise<void> {
  box.click()
  await settle()
}

describe('where notifications go', () => {
  it('groups the kinds and ticks where each goes', async () => {
    await mountApp(RoutesTable, { props: { channels } })
    const text = page()
    expect(text.indexOf(t('channels.routes.groups.storage'))).toBeLessThan(
      text.indexOf(t('channels.routes.groups.folders')),
    )
    expect(cell('pool_trouble', t('channels.routes.inApp')).checked).toBe(true)
    expect(cell('pool_trouble', 'Mail').checked).toBe(true)
    expect(cell('pool_trouble', 'Chat').checked).toBe(false)
    expect(cell('snapshot_failed', t('channels.routes.inApp')).checked).toBe(false)
    expect(text).toContain('weather')
  })

  it('saves each tick for its kind alone', async () => {
    await mountApp(RoutesTable, { props: { channels } })
    await tick(cell('pool_trouble', 'Chat'))
    await tick(cell('pool_trouble', 'Mail'))
    await tick(cell('scrub_finished', t('channels.routes.inApp')))
    expect(called('PUT', '/api/v1/notification-routes').map((call) => call.body)).toEqual([
      { kinds: [{ kind: 'pool_trouble', in_app: true, channels: [1, 2] }] },
      { kinds: [{ kind: 'pool_trouble', in_app: true, channels: [2] }] },
      { kinds: [{ kind: 'scrub_finished', in_app: false, channels: [] }] },
    ])
    expect(cell('pool_trouble', 'Mail').checked).toBe(false)
    expect(cell('scrub_finished', t('channels.routes.inApp')).checked).toBe(false)
  })

  it('says why a tick was not saved and takes it back', async () => {
    on('PUT', '/api/v1/notification-routes', problem(409, 'refused', 'gone'))
    await mountApp(RoutesTable, { props: { channels } })
    await tick(cell('scrub_finished', 'Chat'))
    expect(page()).toContain('gone')
    expect(cell('scrub_finished', 'Chat').checked).toBe(false)
  })

  it('says when they cannot be read', async () => {
    on('GET', '/api/v1/notification-routes', problem(409, 'refused', 'not yours'))
    await mountApp(RoutesTable, { props: { channels } })
    expect(page()).toContain('not yours')
  })
})
