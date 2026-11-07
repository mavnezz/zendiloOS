import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, press, settle } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { NotificationChannel } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ChannelsSection from './ChannelsSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const mail: NotificationChannel = {
  id: 1,
  name: 'Admins',
  target: {
    kind: 'email',
    server: 'smtp.example.org',
    port: 587,
    security: 'starttls',
    username: 'nas',
    from: 'nas@example.org',
    to: ['a@example.org'],
  },
  password_set: true,
  min_level: 'warning',
  language: 'en',
  last_delivery_at: 1_790_000_000,
  last_error: null,
}

const hook: NotificationChannel = {
  id: 2,
  name: 'Chat',
  target: { kind: 'webhook', url: 'https://hooks.example.org/abc', format: 'json' },
  password_set: false,
  min_level: 'error',
  language: 'de',
  last_delivery_at: null,
  last_error: 'timed out',
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/notification-channels', [
    mail,
    hook,
    { ...hook, id: 3, name: 'Odd', target: { kind: 'webhook', url: 'not a url', format: 'json' } },
  ])
  on('POST', '/api/v1/notification-channels/1/test', null)
  on(
    'POST',
    '/api/v1/notification-channels/2/test',
    problem(502, 'delivery_failed', 'connection refused'),
  )
  on('DELETE', '/api/v1/notification-channels/1', problem(409, 'refused', 'in use'))
})

describe('the notification channels', () => {
  it('lists the channels with where they deliver to', async () => {
    await mountApp(ChannelsSection)
    expect(page()).toContain('a@example.org · smtp.example.org')
    expect(page()).toContain('hooks.example.org')
    expect(page()).toContain('not a url')
  })

  it('sends tests and says how they went', async () => {
    await mountApp(ChannelsSection)
    const tests = [...document.querySelectorAll<HTMLButtonElement>('button')].filter((b) =>
      b.textContent?.includes(t('channels.test')),
    )
    tests[0]!.click()
    tests[1]!.click()
    await settle()
    expect(page()).toContain(t('channels.testSent'))
    expect(page()).toContain('connection refused')
    expect(called('POST', '/api/v1/notification-channels/1/test')).toHaveLength(1)
  })

  it('opens the dialog to add or change one, and removes one', async () => {
    const { wrapper } = await mountApp(ChannelsSection)
    await click(new RegExp(t('channels.add')))
    expect(wrapper.findComponent({ name: 'ChannelDialog' }).props('channel')).toBeNull()
    await press(t('desktop.close'))
    const change = [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
      b.textContent?.includes(t('channels.change')),
    )!
    change.click()
    await settle()
    expect(wrapper.findComponent({ name: 'ChannelDialog' }).props('channel')).toMatchObject({
      id: 1,
    })
    await press(t('desktop.close'))

    document
      .querySelectorAll<HTMLButtonElement>(`button[aria-label="${t('channels.remove')}"]`)[0]!
      .click()
    await settle()
    wrapper.findComponent({ name: 'ConfirmDialog' }).vm.$emit('confirm')
    await settle()
    expect(called('DELETE', '/api/v1/notification-channels/1')).toHaveLength(1)
    expect(page()).toContain('in use')
  })
})
