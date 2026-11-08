import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { AcmePlugin, Certificate } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import { job } from '@/shared/testing/fixtures'

import AcmeSection from './AcmeSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const plugin = (installed: boolean): AcmePlugin => ({
  installed,
  providers: [
    {
      api: 'dns_ionos',
      label: 'IONOS',
      variables: [
        { name: 'IONOS_PREFIX', secret: false, required: true },
        { name: 'IONOS_SECRET', secret: true, required: true },
      ],
    },
    {
      api: 'dns_cf',
      label: 'Cloudflare',
      variables: [{ name: 'CF_Token', secret: true, required: true }],
    },
  ],
})

const issued: Certificate = {
  id: 3,
  domains: ['nas.example.org', 'files.example.org'],
  key: 'ec256',
  account_id: 1,
  challenge_id: 2,
  auto_renew: true,
  created_at: 1_790_000_000,
  issued_at: 1_790_000_000,
  not_after: Date.now() / 1000 + 60 * 24 * 3600,
  last_attempt_at: 1_790_000_000,
  last_error: null,
  last_output: ['Creating domain key', 'Cert success.'],
}

/** Opens a reka menu the way a pointer does, and picks `item`. */
async function menu(label: string, item: string): Promise<void> {
  const trigger = document.querySelector(`button[aria-label="${label}"]`)!
  for (const kind of ['pointerdown', 'pointerup', 'click'])
    trigger.dispatchEvent(new MouseEvent(kind, { bubbles: true, button: 0 }))
  await settle()
  const entry = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((element) =>
    element.textContent?.includes(item),
  )
  if (!entry) throw new Error(`no menu item ${item}`)
  entry.click()
  await settle()
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/certificates/plugin', plugin(true))
  on('GET', '/api/v1/certificates/accounts', [
    {
      id: 1,
      name: "Let's Encrypt",
      server: 'letsencrypt',
      email: 'admin@example.org',
      created_at: 0,
    },
  ])
  on('GET', '/api/v1/certificates/challenges', [
    {
      id: 2,
      name: 'IONOS',
      method: 'dns',
      provider: 'dns_ionos',
      values: [
        { name: 'IONOS_PREFIX', secret: false, value: 'prefix' },
        { name: 'IONOS_SECRET', secret: true, value: null },
      ],
      created_at: 0,
    },
  ])
  on('GET', '/api/v1/certificates', [issued])
})

describe('the ACME client plugin', () => {
  it('is installed from Debian in a job', async () => {
    on('GET', '/api/v1/certificates/plugin', plugin(false))
    on('POST', '/api/v1/certificates/plugin/install', job(7, 'certificates.acme_install'))
    await mountApp(AcmeSection)
    expect(page()).toContain(t('appStore.acme.notInstalled'))
    expect(called('GET', '/api/v1/certificates')).toHaveLength(0)
    await click(t('appStore.install'))
    expect(called('POST', '/api/v1/certificates/plugin/install')).toHaveLength(1)
    expect(page()).toContain(t('appStore.acme.installing'))

    useJobsStore().update(
      job(7, 'certificates.acme_install', { state: 'failed', error_detail: 'E: no acme.sh' }),
    )
    await settle()
    expect(page()).toContain('E: no acme.sh')
  })

  it('shows the certificates with their state, and the accounts and challenges', async () => {
    await mountApp(AcmeSection)
    expect(page()).toContain('nas.example.org')
    expect(page()).toContain('+1')
    expect(page()).toContain(t('appStore.acme.autoRenew'))
    expect(page()).toContain('admin@example.org')
    expect(page()).toContain('IONOS')
    expect(page()).not.toContain('s3cr3t')
  })

  it('adds an account and a DNS challenge, keeping a saved secret when left empty', async () => {
    on('POST', '/api/v1/certificates/accounts', { id: 4 })
    on('PUT', '/api/v1/certificates/challenges/2', null)
    await mountApp(AcmeSection)
    const add = [...document.querySelectorAll<HTMLButtonElement>('button')].filter(
      (b) => b.textContent?.trim() === t('appStore.acme.add'),
    )
    add[1]!.click()
    await settle()
    await type('#acme-account-server', 'letsencrypt_staging')
    expect(page()).toContain(t('appStore.acme.account.stagingHint'))
    await type('#acme-account-email', 'ops@example.org')
    await submit('#acme-account')
    expect(called('POST', '/api/v1/certificates/accounts')[0]?.body).toEqual({
      name: "Let's Encrypt",
      server: 'letsencrypt_staging',
      email: 'ops@example.org',
    })

    await menu('IONOS', t('appStore.acme.edit'))
    const secret = document.querySelector<HTMLInputElement>('#acme-IONOS_SECRET')!
    expect(secret.type).toBe('password')
    expect(secret.placeholder).toBe(t('appStore.acme.challenge.secretSet'))
    await type('#acme-IONOS_PREFIX', 'other')
    await submit('#acme-challenge')
    expect(called('PUT', '/api/v1/certificates/challenges/2')[0]?.body).toEqual({
      name: 'IONOS',
      method: 'dns',
      provider: 'dns_ionos',
      values: { IONOS_PREFIX: 'other', IONOS_SECRET: '' },
    })
  })

  it('issues a certificate and follows what acme.sh writes', async () => {
    on(
      'POST',
      '/api/v1/certificates/3/issue',
      job(9, 'certificates.issue', { target: 'nas.example.org' }),
    )
    await mountApp(AcmeSection)
    await menu('nas.example.org', t('appStore.acme.renew'))
    expect(called('POST', '/api/v1/certificates/3/issue')).toHaveLength(1)
    expect(page()).toContain(t('appStore.acme.logWaiting'))

    useJobsStore().update(
      job(9, 'certificates.issue', {
        target: 'nas.example.org',
        output: ['[Sat Nov  7 12:00:00 UTC 2026] Verifying: nas.example.org'],
      }),
    )
    await settle()
    expect(document.querySelector('[data-testid="acme-output"]')?.textContent).toContain(
      'Verifying',
    )
    useJobsStore().update(
      job(9, 'certificates.issue', {
        target: 'nas.example.org',
        state: 'failed',
        error_detail: 'acme.sh: Verify error',
      }),
    )
    await settle()
    expect(page()).toContain(t('appStore.acme.logFailed'))
    expect(page()).toContain('acme.sh: Verify error')
  })

  it('shows the last log, and says why something cannot be deleted', async () => {
    on(
      'DELETE',
      '/api/v1/certificates/3',
      problem(409, 'in_use', 'still used by the web interface'),
    )
    await mountApp(AcmeSection)
    await menu('nas.example.org', t('appStore.acme.log'))
    expect(document.querySelector('[data-testid="acme-output"]')?.textContent).toContain(
      'Cert success.',
    )
    await click(t('docker.deploy.ok'))

    await menu('nas.example.org', t('appStore.acme.delete'))
    await click(t('appStore.acme.delete'))
    expect(called('DELETE', '/api/v1/certificates/3')).toHaveLength(1)
    expect(page()).toContain('still used by the web interface')
  })

  it('adds a certificate and issues it at once', async () => {
    on('POST', '/api/v1/certificates', { ...issued, id: 5, issued_at: null, not_after: null })
    on(
      'POST',
      '/api/v1/certificates/5/issue',
      job(10, 'certificates.issue', { target: '*.example.org' }),
    )
    await mountApp(AcmeSection)
    const add = [...document.querySelectorAll<HTMLButtonElement>('button')].find(
      (b) => b.textContent?.trim() === t('appStore.acme.add'),
    )
    add!.click()
    await settle()
    await type('#acme-certificate-names', '*.Example.org\nexample.org')
    await type('#acme-certificate-key', 'rsa4096')
    await submit('#acme-certificate')
    expect(called('POST', '/api/v1/certificates')[0]?.body).toEqual({
      domains: ['*.example.org', 'example.org'],
      key: 'rsa4096',
      account_id: 1,
      challenge_id: 2,
      auto_renew: true,
    })
    expect(called('POST', '/api/v1/certificates/5/issue')).toHaveLength(1)
  })
})
