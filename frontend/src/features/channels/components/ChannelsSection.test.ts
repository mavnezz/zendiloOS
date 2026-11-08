import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, press, settle } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { NotificationChannel, NotificationRoute } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ChannelsSection from './ChannelsSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const mail: NotificationChannel = {
  id: 1,
  name: 'Admins',
  target: { kind: 'email', accounts: [2, 3, 9], to: ['a@example.org'] },
  recipients: [
    { account_id: 2, address: 'anna@example.org', problem: null },
    { account_id: 3, address: null, problem: 'disabled' },
    { account_id: 9, address: null, problem: 'gone' },
    { account_id: null, address: 'a@example.org', problem: null },
  ],
  language: 'en',
  last_delivery_at: 1_790_000_000,
  last_error: null,
}

const hook: NotificationChannel = {
  id: 2,
  name: 'Chat',
  target: { kind: 'webhook', url: 'https://hooks.example.org/abc', format: 'json' },
  recipients: [],
  language: 'de',
  last_delivery_at: 1_790_000_000,
  last_error: 'timed out',
}

const account = (id: number, name: string) => ({
  id,
  name,
  role: 'user',
  disabled: false,
  created_at: 0,
  updated_at: 0,
  password_changed_at: 0,
  password_change_required: false,
  password_locked: false,
})

const routes: NotificationRoute[] = [
  { kind: 'pool_trouble', group: 'storage', level: 'warning', in_app: true, channels: [1, 2] },
  { kind: 'updates_failed', group: 'system', level: 'error', in_app: true, channels: [] },
]

beforeEach(() => {
  reset()
  on('GET', '/api/v1/notification-channels', [
    mail,
    hook,
    { ...hook, id: 3, name: 'Odd', target: { kind: 'webhook', url: 'not a url', format: 'json' } },
  ])
  on('GET', '/api/v1/notification-mail-server', {
    server: 'smtp.example.org',
    port: 587,
    security: 'starttls',
    username: 'nas',
    from: 'nas@example.org',
    password_set: true,
  })
  on('GET', '/api/v1/notification-routes', routes)
  on('GET', '/api/v1/accounts', [account(2, 'anna'), account(3, 'ben')])
  on('POST', '/api/v1/notification-channels/1/test', null)
  on(
    'POST',
    '/api/v1/notification-channels/2/test',
    problem(502, 'delivery_failed', 'connection refused'),
  )
  on('DELETE', '/api/v1/notification-channels/1', problem(409, 'refused', 'in use'))
})

describe('the notification settings', () => {
  it('show the mail server, and the channels with where they deliver to', async () => {
    await mountApp(ChannelsSection)
    expect(page()).toContain('smtp.example.org:587')
    expect(page()).toContain('STARTTLS · nas@example.org')
    const recipients = [
      ...document.querySelectorAll(`ul[aria-label="${t('channels.recipients')}"] li`),
    ].map((entry) => entry.textContent?.replace(/\s+/g, ' ').trim())
    expect(recipients).toEqual([
      'anna (anna@example.org)',
      `ben (${t('channels.problems.disabled')})`,
      `${t('channels.account', { id: 9 })} (${t('channels.problems.gone')})`,
      'a@example.org',
    ])
    expect(page()).toContain('hooks.example.org')
    expect(page()).toContain('not a url')
    expect(page()).toContain('timed out')
  })

  it('say when there is no mail server for the email channels', async () => {
    on('GET', '/api/v1/notification-mail-server', null)
    await mountApp(ChannelsSection)
    expect(page()).toContain(t('channels.mailServer.none'))
    await click(new RegExp(t('channels.mailServer.setUp')))
    expect(page()).toContain(t('channels.mailServer.dialogHint'))
  })

  it('show where each kind goes, a column per channel', async () => {
    await mountApp(ChannelsSection)
    expect(page()).toContain(t('channels.routes.groups.storage'))
    expect(page()).toContain(t('channels.routes.kinds.updates_failed'))
    const cell = (to: string) =>
      document.querySelector<HTMLInputElement>(
        `input[aria-label="${t('channels.routes.cell', { kind: t('channels.routes.kinds.pool_trouble'), to })}"]`,
      )!
    expect(cell('Admins').checked).toBe(true)
    expect(cell('Odd').checked).toBe(false)
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

  it('opens the dialogs to change the server or a channel, and removes one', async () => {
    const { wrapper } = await mountApp(ChannelsSection)
    const changes = () =>
      [...document.querySelectorAll<HTMLButtonElement>('button')].filter((b) =>
        b.textContent?.includes(t('channels.change')),
      )
    changes()[0]!.click()
    await settle()
    expect(wrapper.findComponent({ name: 'MailServerDialog' }).props('server')).toMatchObject({
      server: 'smtp.example.org',
    })
    await press(t('desktop.close'))

    await click(new RegExp(t('channels.add')))
    expect(wrapper.findComponent({ name: 'ChannelDialog' }).props('channel')).toBeNull()
    await press(t('desktop.close'))
    changes()[1]!.click()
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
