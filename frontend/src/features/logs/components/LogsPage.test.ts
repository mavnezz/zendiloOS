import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, settle } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'
import { i18n } from '@/shared/i18n'

import LogsPage from './LogsPage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)
async function open(page: 'journal' | 'audit') {
  return mountApp(LogsPage, { props: { page } })
}

const levels = ['emergency', 'error', 'warning', 'notice', 'debug'] as const

beforeEach(() => {
  reset()
  on('GET', '/api/v1/logs/journal', (call: { query: Record<string, string> }) =>
    call.query.before
      ? {
          entries: [
            {
              at_micros: 1_000_000,
              message: 'an older line',
              priority: 'info',
              unit: null,
              identifier: null,
              pid: null,
            },
          ],
          older: null,
        }
      : {
          entries: levels.map((priority, index) => ({
            at_micros: 1_790_000_000_000_000 + index,
            message: `${priority} from ${call.query.source}`,
            priority,
            unit: 'smbd.service',
            identifier: 'smbd',
            pid: 100 + index,
          })),
          older: 'cursor-1',
        },
  )
  on('GET', '/api/v1/logs/audit', (call: { query: Record<string, string> }) =>
    call.query.before
      ? {
          entries: [{ id: 1, at: 1, action: 'auth.logout', outcome: 'success', actor: 'alice' }],
          older: null,
        }
      : {
          entries: [
            {
              id: 3,
              at: 1_790_000_000,
              action: 'auth.login',
              outcome: 'success',
              actor: 'alice',
              client_ip: '192.168.1.5',
              target: null,
            },
            {
              id: 2,
              at: 1_790_000_000,
              action: 'storage.pool_destroy',
              outcome: 'denied',
              actor: 'bob',
              target: 'tank',
              reason: 'forbidden',
            },
            {
              id: 4,
              at: 1_790_000_000,
              action: 'brand.new',
              outcome: 'unheard',
              actor: null,
              target: null,
            },
          ],
          older: 2,
        },
  )
})

describe('the logs', () => {
  it('read the journal by source and level, and older entries on request', async () => {
    await open('journal')
    expect(page()).toContain(t('logs.sections.journal'))
    expect(page()).toContain('error from system')
    const [source, priority] = document.querySelectorAll<HTMLSelectElement>('select')
    source!.value = 'sharing'
    source!.dispatchEvent(new Event('change'))
    priority!.value = 'debug'
    priority!.dispatchEvent(new Event('change'))
    await settle()
    expect(called('GET', '/api/v1/logs/journal').at(-1)?.query).toMatchObject({
      source: 'sharing',
      priority: 'debug',
    })
    await click(new RegExp(t('logs.older')))
    expect(page()).toContain('an older line')
    await click(/Refresh/)
  })

  it('read the audit log', async () => {
    await open('audit')
    expect(page()).toContain(t('logs.sections.audit'))
    expect(page()).toContain(t('logs.descriptions.audit'))
    expect(page()).toContain('tank')
    expect(page()).toContain('brand.new')
    expect(page()).toContain('unheard')
    await click(new RegExp(t('logs.older')))
    expect(called('GET', '/api/v1/logs/audit').at(-1)?.query).toMatchObject({ before: '2' })
    await click(/Refresh/)
  })
})
