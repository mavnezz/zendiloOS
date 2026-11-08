import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { Certificate, WebServerSetup } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import WebInterfacePage from './WebInterfacePage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const now = Date.now() / 1000
const certificate = (id: number, name: string, notAfter: number | null): Certificate => ({
  id,
  domains: [name],
  key: 'ec256',
  account_id: 1,
  challenge_id: 1,
  auto_renew: true,
  created_at: 0,
  issued_at: notAfter ? 0 : null,
  not_after: notAfter,
  last_attempt_at: null,
  last_error: null,
  last_output: [],
})
const setup = (pending: WebServerSetup['pending'] = null): WebServerSetup => ({
  current: { http_port: 80, https_port: 443, certificate_id: null },
  pending,
  confirm_within: 90,
})

beforeEach(() => {
  reset()
  on('GET', '/api/v1/system/web-server', setup())
  on('GET', '/api/v1/certificates', [
    certificate(3, 'nas.example.org', now + 30 * 24 * 3600),
    certificate(4, 'old.example.org', now - 3600),
    certificate(5, 'new.example.org', null),
  ])
})

const apply = () => click(t('common.apply'))

describe('the web interface page', () => {
  it('moves HTTPS to another port with a certificate, and is confirmed there', async () => {
    on('PUT', '/api/v1/system/web-server', setup())
    on('POST', '/api/v1/system/web-server/confirm', null)
    await mountApp(WebInterfacePage, { props: { canManage: true } })
    const options = [...document.querySelectorAll('#web-certificate option')].map(
      (o) => o.textContent,
    )
    expect(options).toHaveLength(2)
    expect(options[1]).toContain('nas.example.org')

    await type('#web-https', '8443')
    await type('#web-certificate', '3')
    document.querySelector<HTMLButtonElement>('#web-http')!.click()
    await settle()
    on(
      'GET',
      '/api/v1/system/web-server',
      setup({
        settings: { http_port: null, https_port: 8443, certificate_id: 3 },
        revert_at: now + 90,
      }),
    )
    await apply()
    expect(called('PUT', '/api/v1/system/web-server')[0]?.body).toEqual({
      http_port: null,
      https_port: 8443,
      certificate_id: 3,
    })
    expect(page()).toContain(t('webServer.pending.title'))
    expect(document.querySelector<HTMLAnchorElement>('[role="status"] a')?.href).toBe(
      `https://${window.location.hostname}:8443/`,
    )
    on('GET', '/api/v1/system/web-server', {
      ...setup(),
      current: { http_port: null, https_port: 8443, certificate_id: 3 },
    })
    await click(t('webServer.pending.confirm'))
    expect(called('POST', '/api/v1/system/web-server/confirm')).toHaveLength(1)
    expect(page()).not.toContain(t('webServer.pending.title'))
  })

  it('refuses ports it cannot use before asking, and says what the server refused', async () => {
    on('PUT', '/api/v1/system/web-server', problem(409, 'port_in_use', '8080'))
    await mountApp(WebInterfacePage, { props: { canManage: true } })
    await type('#web-https', '80')
    expect(page()).toContain(t('webServer.samePorts'))
    await type('#web-https', '70000')
    expect(page()).toContain(t('webServer.invalidPort'))
    await type('#web-https', '8080')
    await apply()
    expect(page()).toContain(t('errors.port_in_use'))
    expect(page()).toContain('8080')
  })

  it('only shows the settings to those who may not change them', async () => {
    await mountApp(WebInterfacePage, { props: { canManage: false } })
    expect(document.querySelector<HTMLInputElement>('#web-https')?.disabled).toBe(true)
    expect(called('GET', '/api/v1/certificates')).toHaveLength(0)
    expect(page()).toContain(t('webServer.own'))
  })
})
