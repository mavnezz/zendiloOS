import { describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

import { ApiProblem, configureApi, csrfToken } from '@/shared/api/client'
import { LiveClient, type SocketLike } from '@/shared/api/live'
import { i18n, problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { wording } from '@/shared/notices/store'

/** A socket that is open and keeps what was sent. */
function socket(): SocketLike & { sent: string[] } {
  return {
    readyState: 1,
    onopen: null,
    onmessage: null,
    onclose: null,
    sent: [],
    send(data: string) {
      this.sent.push(data)
    },
    close: vi.fn(),
  }
}

describe('the live connection', () => {
  it('hands each message to the listeners of its topic, and ignores what it cannot read', () => {
    const opened = socket()
    const client = new LiveClient(
      () => 'ws://nas/api/v1/ws',
      () => opened,
    )
    const metrics = vi.fn()
    const notices = vi.fn()
    client.subscribe('metrics', metrics)
    client.subscribe('notifications', notices)
    client.connect()
    opened.onopen?.(new Event('open'))
    const deliver = (data: unknown) => opened.onmessage?.({ data } as MessageEvent)

    deliver(JSON.stringify({ type: 'metrics', data: { cpu: 1 } }))
    deliver(JSON.stringify({ type: 'notification', data: { id: 1 } }))
    deliver(JSON.stringify({ type: 'job', data: { id: 2 } }))
    deliver('not json')
    deliver(new ArrayBuffer(2))

    expect(metrics).toHaveBeenCalledWith({ cpu: 1 })
    expect(notices).toHaveBeenCalledWith({ id: 1 })
    expect(opened.sent.map((s) => JSON.parse(s).topic)).toEqual(['metrics', 'notifications'])
    client.onSessionEnded()
    client.disconnect()
  })

  it('reaches the server where the page came from', async () => {
    const { live } = await import('@/shared/api/live')
    const url = (live as unknown as { url: () => string }).url()
    expect(url).toMatch(/^wss?:\/\/.*\/api\/v1\/ws$/)
  })
})

describe('the API client', () => {
  it('reads the CSRF token from the session it was given', () => {
    configureApi({ csrfToken: () => 'csrf-7', onUnauthenticated: () => undefined })
    expect(csrfToken()).toBe('csrf-7')
    configureApi({ csrfToken: () => undefined, onUnauthenticated: () => undefined })
  })
})

describe('why something failed', () => {
  it('is said in words, with the server’s detail where it names what', () => {
    i18n.global.locale.value = 'en'
    expect(problemMessage(new Error('x'))).toContain('client')
    expect(problemMessage(new ApiProblem(429, 'too_many_attempts', undefined, 120))).toContain(
      '2 minutes',
    )
    expect(problemMessage(new ApiProblem(429, 'too_many_attempts'))).toContain('1 minutes')
    expect(problemMessage(new ApiProblem(422, 'invalid_input', 'too short'))).toBe('too short')
    expect(problemMessage(new ApiProblem(409, 'pool_in_use', 'share photos'))).toContain(
      '(share photos)',
    )
    expect(problemMessage(new ApiProblem(409, 'name_taken', 'ignored'))).not.toContain('ignored')
    expect(problemMessage(new ApiProblem(418, 'teapot'))).toContain('teapot')
  })
})

describe('notifications', () => {
  it('show as toasts for a while, and can be marked read and cleared', () => {
    vi.useFakeTimers()
    setActivePinia(createPinia())
    const notifications = useNotifications()
    notifications.notify('info', 'One')
    notifications.notify('error', 'Two', 'why', { toast: false })
    expect(notifications.toasts.map((n) => n.title)).toEqual(['One'])
    expect(notifications.unread).toBe(2)
    vi.advanceTimersByTime(6000)
    expect(notifications.toasts).toEqual([])
    notifications.markAllRead()
    expect(notifications.unread).toBe(0)
    notifications.clear()
    expect(notifications.items).toEqual([])
    vi.useRealTimers()
  })
})

describe('server notifications', () => {
  it('name pool and disk states in words, and fall back to their code', () => {
    i18n.global.locale.value = 'en'
    const notice = (code: string, params: Record<string, string> = {}) =>
      ({ id: 1, code, level: 'warning', params, created_at: 0, read: false }) as never
    expect(wording(notice('pool_trouble', { pool: 'tank', state: 'degraded' })).title).toBeTruthy()
    expect(wording(notice('disk_health', { disk: 'sda', health: 'failing' })).title).toBeTruthy()
    expect(wording(notice('disk_health', { disk: 'sda', health: 'warning' })).title).toBeTruthy()
    expect(wording(notice('never_heard_of')).title).toBe('never_heard_of')
  })
})
