import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { AuditRecord } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import AuditLog from './AuditLog.vue'
import { auditCsv, csvField } from './csv'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function record(id: number, extra: Partial<AuditRecord> = {}): AuditRecord {
  return {
    id,
    at: 1_790_000_000 + id,
    actor: 'alice',
    client_ip: '192.168.1.5',
    action: 'vms.create',
    target: `vm${id}`,
    outcome: 'success',
    reason: null,
    request_id: null,
    ...extra,
  }
}

/** Pages of two, newest first: 6 5 | 4 3 | 2 1. */
function pages(call: { query: Record<string, string> }) {
  const all = [6, 5, 4, 3, 2, 1]
    .map((id) => record(id))
    .filter((entry) => !call.query.search || entry.target!.includes(call.query.search))
  const before = call.query.before ? Number(call.query.before) : Infinity
  const limit = Number(call.query.limit)
  const entries = all.filter((entry) => entry.id < before).slice(0, limit)
  const older = entries.length === limit && entries.at(-1)!.id > 1 ? entries.at(-1)!.id : null
  return { entries, older }
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/logs/audit', pages)
})

afterEach(() => vi.useRealTimers())

describe('the audit log', () => {
  it('shows an app’s entries a page at a time, older and newer', async () => {
    await mountApp(AuditLog, { props: { prefix: 'vms.', size: 2, action: () => 'Made' } })
    expect(called('GET', '/api/v1/logs/audit')[0]?.query).toMatchObject({
      prefix: 'vms.',
      limit: '2',
    })
    expect(page()).toContain('vm6')
    expect(page()).toContain('Made')
    expect(page()).toContain(t('logs.page', { n: 1 }))
    await click(new RegExp(`^${t('logs.olderPage')}`))
    expect(page()).toContain('vm4')
    expect(page()).not.toContain('vm6')
    expect(called('GET', '/api/v1/logs/audit').at(-1)?.query).toMatchObject({ before: '5' })
    await click(new RegExp(t('logs.newer')))
    expect(page()).toContain('vm6')
    expect(page()).toContain(t('logs.page', { n: 1 }))
  })

  it('searches on the server a moment after typing stops', async () => {
    await mountApp(AuditLog, { props: { prefix: 'vms.' } })
    await type('[data-testid="audit-search"]', ' vm3 ')
    await new Promise((resolve) => setTimeout(resolve, 350))
    await settle()
    expect(called('GET', '/api/v1/logs/audit').at(-1)?.query).toMatchObject({
      search: 'vm3',
      prefix: 'vms.',
    })
    expect(document.querySelectorAll('[data-testid="audit-row"]')).toHaveLength(1)
    await type('[data-testid="audit-search"]', 'nothing')
    await new Promise((resolve) => setTimeout(resolve, 350))
    await settle()
    expect(page()).toContain(t('logs.empty'))
  })

  it('exports every entry the search takes as CSV', async () => {
    const created: Blob[] = []
    Object.assign(URL, {
      createObjectURL: vi.fn((blob: Blob) => {
        created.push(blob)
        return 'blob:audit'
      }),
      revokeObjectURL: vi.fn(),
    })
    const clicked = vi
      .spyOn(HTMLAnchorElement.prototype, 'click')
      .mockImplementation(() => undefined)
    await mountApp(AuditLog, { props: { prefix: 'vms.', size: 2, file: 'vms-log' } })
    document.querySelector<HTMLButtonElement>('[data-testid="audit-export"]')!.click()
    await settle()
    expect(clicked).toHaveBeenCalled()
    const text = await created[0]!.text()
    const lines = text.replace(/^﻿/, '').trim().split('\r\n')
    expect(lines).toHaveLength(7)
    expect(lines[0]).toBe(
      [
        t('logs.time'),
        t('logs.actor'),
        t('logs.action'),
        t('logs.target'),
        t('logs.outcome'),
        t('logs.address'),
      ]
        .map((heading) => `"${heading}"`)
        .join(','),
    )
    expect(lines[1]).toContain('"alice"')
    expect(lines[6]).toContain('"vm1"')
    const exports = called('GET', '/api/v1/logs/audit').filter((call) => call.query.limit === '500')
    expect(exports[0]?.query).toMatchObject({ prefix: 'vms.' })
    clicked.mockRestore()
  })

  it('says why it cannot be read', async () => {
    on('GET', '/api/v1/logs/audit', problem(403, 'forbidden'))
    await mountApp(AuditLog)
    expect(page()).toContain(t('errors.forbidden'))
  })
})

describe('the audit log as CSV', () => {
  it('quotes every field and runs no formula', () => {
    expect(csvField('plain')).toBe('"plain"')
    expect(csvField('say "hi"')).toBe('"say ""hi"""')
    expect(csvField(null)).toBe('""')
    expect(csvField(7)).toBe('"7"')
    for (const formula of ['=1+1', '+1', '-1', '@SUM(A1)', '\tx', '\rx']) {
      expect(csvField(formula).startsWith(`"'`), formula).toBe(true)
    }
    const text = auditCsv(
      [record(1, { reason: 'wrong_password', actor: null, client_ip: null })],
      { time: 'T', actor: 'A', action: 'W', target: 'G', outcome: 'O', address: 'F' },
      { action: (r) => r.action, outcome: (r) => r.outcome, time: () => 'then' },
    )
    expect(text).toBe(
      '"T","A","W","G","O","F"\r\n"then","","vms.create","vm1 · wrong_password","success",""\r\n',
    )
  })
})
