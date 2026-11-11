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
  waiting: [],
  failed: [],
}

const hook: NotificationChannel = {
  id: 2,
  name: 'Chat',
  target: { kind: 'webhook', url: 'https://hooks.example.org/abc', format: 'json' },
  recipients: [],
  language: 'de',
  last_delivery_at: 1_790_000_000,
  last_error: 'timed out',
  waiting: [],
  failed: [],
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

  it('show each channel as a card of a grid, each with its own actions', async () => {
    const { wrapper } = await mountApp(ChannelsSection)
    const grid = document.querySelector<HTMLElement>('[data-testid="channel-grid"]')!
    expect(grid.className).toContain('grid-cols-[repeat(auto-fill,minmax(18rem,1fr))]')
    const cards = [...grid.children] as HTMLElement[]
    expect(cards.map((card) => card.querySelector('.font-medium')?.textContent?.trim())).toEqual([
      'Admins',
      'Chat',
      'Odd',
    ])
    const actions = (card: HTMLElement) => [...card.querySelectorAll('button')]
    for (const card of cards) {
      expect(
        actions(card).map((b) => b.textContent?.trim() || b.getAttribute('aria-label')),
      ).toEqual([t('channels.test'), t('channels.change'), t('channels.remove')])
    }

    actions(cards[1]!)[0]!.click()
    await settle()
    expect(called('POST', '/api/v1/notification-channels/2/test')).toHaveLength(1)
    expect(called('POST', '/api/v1/notification-channels/1/test')).toHaveLength(0)
    expect(cards[1]!.textContent).toContain('connection refused')

    actions(cards[2]!)[1]!.click()
    await settle()
    expect(wrapper.findComponent({ name: 'ChannelDialog' }).props('channel')).toMatchObject({
      id: 3,
    })
    await press(t('desktop.close'))

    actions(cards[1]!)[2]!.click()
    await settle()
    expect(page()).toContain(t('channels.removeTitle', { name: 'Chat' }))
  })

  it('keep the mail server to one quiet line with its own change', async () => {
    const { wrapper } = await mountApp(ChannelsSection)
    const line = document.querySelector<HTMLElement>('[data-testid="mail-server"]')!
    expect(line.textContent?.replace(/\s+/g, ' ')).toContain(
      `${t('channels.mailServer.title')}: smtp.example.org:587 · STARTTLS · nas@example.org`,
    )
    const buttons = line.querySelectorAll('button')
    expect([...buttons].map((b) => b.textContent?.trim())).toEqual([t('channels.change')])
    buttons[0]!.click()
    await settle()
    expect(wrapper.findComponent({ name: 'MailServerDialog' }).props('open')).toBe(true)
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

  it('say what waits to go out on a channel and what never went out', async () => {
    const degraded = {
      id: 7,
      at: 1_790_000_000,
      level: 'warning' as const,
      code: 'pool_trouble',
      params: { pool: 'tank', state: 'degraded' },
    }
    const recovered = { ...degraded, id: 8, level: 'success' as const, code: 'pool_recovered' }
    on('GET', '/api/v1/notification-channels', [
      {
        ...hook,
        waiting: [
          {
            notification: degraded,
            attempts: 3,
            at: 1_790_000_600,
            error: 'Temporary failure in name resolution',
          },
          { notification: recovered, attempts: 0, at: 1_790_000_600, error: null },
        ],
        failed: [
          {
            notification: degraded,
            attempts: 1,
            at: 1_789_000_000,
            error: 'the server answered 401 Unauthorized',
          },
        ],
      },
      mail,
    ])
    await mountApp(ChannelsSection)
    const text = (id: string) =>
      document.querySelector(`[data-testid="${id}"]`)?.textContent?.replace(/\s+/g, ' ') ?? ''
    expect(text('channel-waiting')).toContain(i18n.global.t('channels.queue.tries', { n: 3 }, 3))
    expect(text('channel-waiting')).toContain('Temporary failure in name resolution')
    expect(text('channel-waiting')).toContain(
      i18n.global.t('notices.pool_recovered.title', { pool: 'tank' }),
    )
    expect(text('channel-waiting')).not.toContain(
      i18n.global.t('channels.queue.tries', { n: 0 }, 0),
    )
    expect(text('channel-failed')).toContain(i18n.global.t('channels.queue.failed', { n: 1 }, 1))
    expect(text('channel-failed')).toContain('the server answered 401 Unauthorized')
    expect(document.querySelectorAll('[data-testid="channel-waiting"]')).toHaveLength(1)
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
