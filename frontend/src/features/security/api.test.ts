import { describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, press, settle, submit, type } from '@/desktop/testing/mount'
import { called, on, reset } from '@/desktop/testing/server'
import { i18n } from '@/shared/i18n'

import AccountActivityDialog from './components/AccountActivityDialog.vue'
import SecurityPage from './components/SecurityPage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/desktop/testing/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const settings = {
  idle_minutes: 30,
  account_lock: { failures: 5, window_minutes: 15 },
  auto_block: { failures: 10, window_minutes: 5, release_after_days: 1 },
  allowed: ['192.168.1.0/24'],
}

describe('security', () => {
  it('lists sessions and logins and ends a session', async () => {
    reset()
    on('GET', '/api/v1/security/sessions', [
      { id: 1, account: 'alice', address: '192.168.1.5', user_agent: 'Mozilla/5.0 (X11; Linux x86_64) Firefox/140.0', created_at: 1_790_000_000, last_seen_at: 1_790_000_100, current: true },
      { id: 2, account: 'bob', address: null, user_agent: null, created_at: 1_790_000_000, last_seen_at: 1_790_000_100, current: false },
    ])
    on('DELETE', '/api/v1/security/sessions/2', null)
    on('GET', '/api/v1/logs/audit', {
      entries: [
        { id: 3, at: 1_790_000_000, action: 'auth.login', outcome: 'success', actor: 'alice', client_ip: '192.168.1.5' },
        { id: 2, at: 1_790_000_000, action: 'auth.login', outcome: 'failure', actor: 'bob', reason: 'wrong_password' },
        { id: 1, at: 1_790_000_000, action: 'auth.login', outcome: 'failure', actor: 'eve', reason: 'something_else' },
      ],
      older: null,
    })
    const { wrapper } = await mountApp(AccountActivityDialog, { props: { open: false } })
    await wrapper.setProps({ open: true })
    await settle()
    expect(page()).toContain('Firefox')
    expect(page()).toContain('bob')
    await press(t('security.activity.end'))
    expect(called('DELETE', '/api/v1/security/sessions/2')).toHaveLength(1)

    wrapper.findComponent({ name: 'PageTabs' }).vm.$emit('update:modelValue', 'logins')
    await settle()
    expect(page()).toContain(t('security.activity.success'))
    expect(page()).toContain(t('security.activity.reasons.wrong_password'))
    expect(page()).toContain(t('security.activity.failed'))
  })

  it('saves settings, the oldest TLS and blocks through the API', async () => {
    reset()
    on('GET', '/api/v1/security/settings', settings)
    on('PUT', '/api/v1/security/settings', (call: { body: unknown }) => call.body)
    on('GET', '/api/v1/system/tls', { minimum: 'tls1_2' })
    on('PUT', '/api/v1/system/tls', (call: { body: unknown }) => call.body)
    on('GET', '/api/v1/security/blocked', [{ address: '203.0.113.9', blocked_at: 1_800_000_000, until: null, automatic: true }])
    on('POST', '/api/v1/security/blocked', { address: '198.51.100.7', blocked_at: 1, until: null, automatic: false })
    on('POST', '/api/v1/security/unblock', null)
    const { wrapper } = await mountApp(SecurityPage)
    await type('#security-idle', '45')
    wrapper.findComponent({ name: 'PageFooter' }).vm.$emit('apply')
    await settle()
    expect(called('PUT', '/api/v1/security/settings')[0]?.body).toMatchObject({ idle_minutes: 45 })

    await type('#security-tls', 'tls1_3')
    wrapper.findComponent({ name: 'PageFooter' }).vm.$emit('apply')
    await settle()
    expect(called('PUT', '/api/v1/system/tls')[0]?.body).toEqual({ minimum: 'tls1_3' })

    await click(new RegExp(t('security.manageBlocks')))
    expect(page()).toContain('203.0.113.9')
    await type(`[role="dialog"] input[aria-label="${t('security.blocks.address')}"]`, '198.51.100.7')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/security/blocked')[0]?.body).toEqual({ address: '198.51.100.7' })
    await press(t('security.blocks.lift'))
    expect(called('POST', '/api/v1/security/unblock')[0]?.body).toEqual({ address: '203.0.113.9' })
  })
})
