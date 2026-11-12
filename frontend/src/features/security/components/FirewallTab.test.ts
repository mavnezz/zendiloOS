import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { FirewallSettings, FirewallSetup } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import {
  click,
  labelled,
  mountApp,
  page,
  press,
  settle,
  submit,
  type,
} from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import FirewallTab from './FirewallTab.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const settings = (change: Partial<FirewallSettings> = {}): FirewallSettings => ({
  enabled: true,
  fallback: 'allow',
  rules: [
    {
      enabled: true,
      interface: null,
      service: 'web_interface',
      source: '192.168.1.0/24',
      action: 'allow',
    },
    {
      enabled: true,
      interface: 'enp1s0',
      service: 'ports',
      protocol: 'tcp',
      ports: '8000-8100',
      source: 'any',
      action: 'deny',
    },
  ],
  ...change,
})

const setup = (change: Partial<FirewallSetup> = {}): FirewallSetup => ({
  current: settings(),
  pending: null,
  confirm_within: 90,
  blocked: [],
  services: [
    { key: 'web_interface', service: 'web_interface', on: true, tcp: '80, 443', udp: '' },
    { key: 'smb', service: 'smb', on: true, tcp: '139, 445', udp: '137-138' },
    { key: 'project:media', service: 'project', project: 'media', on: true, tcp: '8096', udp: '' },
  ],
  interfaces: ['enp1s0'],
  local_networks: ['192.168.1.0/24'],
  restored: null,
  ...change,
})

/** Moves the rule at `index` up one place. */
async function up(index: number): Promise<void> {
  const buttons = document.querySelectorAll<HTMLButtonElement>(
    `button[aria-label="${t('firewall.moveUp')}"]`,
  )
  buttons[index]!.click()
  await settle()
}

/** Picks the radio button labelled `text`. */
async function choose(text: string): Promise<void> {
  const label = [...document.querySelectorAll('label')].find((l) => l.textContent?.trim() === text)
  label!.querySelector<HTMLButtonElement>('button')!.click()
  await settle()
}

/** The rules' rows, as text. */
function rows(): string[] {
  return [...document.querySelectorAll('tr[data-rule]')].map((row) =>
    (row.textContent ?? '').replace(/\s+/g, ' ').trim(),
  )
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/security/firewall', setup())
})

describe('the firewall tab', () => {
  it('shows the rules in their order with their ports', async () => {
    await mountApp(FirewallTab)
    expect(rows()).toHaveLength(2)
    expect(rows()[0]).toContain('Web interface')
    expect(rows()[0]).toContain('80, 443')
    expect(rows()[0]).toContain('192.168.1.0/24')
    expect(rows()[1]).toContain('enp1s0')
    expect(rows()[1]).toContain('8000-8100')
    expect(rows()[1]).toContain('Deny')
    expect(document.querySelector('footer')).toBeNull()
  })

  it('adds a rule, orders the rules and applies them', async () => {
    on('PUT', '/api/v1/security/firewall', (call: { body: unknown }) => ({
      ...setup(),
      pending: { settings: call.body, revert_at: Date.now() / 1000 + 90 },
    }))
    await mountApp(FirewallTab)
    await click(t('firewall.add'))
    await type('#firewall-rule-service', 'project')
    await type('#firewall-rule-source', 'custom')
    await type('#firewall-rule-custom', 'nonsense')
    expect(page()).toContain(t('firewall.rule.invalidSource'))
    await type('#firewall-rule-custom', '10.0.0.0/8')
    await submit('#firewall-rule')
    expect(rows()).toHaveLength(3)
    expect(rows()[2]).toContain('Docker project media')
    expect(rows()[2]).toContain('10.0.0.0/8')

    // The new rule to the top, a step at a time.
    await up(2)
    await up(1)
    expect(rows()[0]).toContain('Docker project media')
    await click(t('common.apply'))
    const sent = called('PUT', '/api/v1/security/firewall')[0]!.body as {
      rules: { service: string; project: string | null; source: string }[]
      lock_out: boolean
    }
    expect(sent.lock_out).toBe(false)
    expect(sent.rules.map((r) => r.service)).toEqual(['project', 'web_interface', 'ports'])
    expect(sent.rules[0]).toMatchObject({ project: 'media', source: '10.0.0.0/8' })
  })

  it('asks before rules shut out this browser, and keeps them when told', async () => {
    let tries = 0
    on('PUT', '/api/v1/security/firewall', () => {
      tries += 1
      return tries === 1 ? problem(409, 'would_lock_out') : setup()
    })
    await mountApp(FirewallTab)
    await press(t('firewall.remove'))
    await click(t('common.apply'))
    expect(page()).toContain(t('firewall.lockOut.title'))
    await click(t('firewall.lockOut.confirm'))
    const sent = called('PUT', '/api/v1/security/firewall')
    expect(sent).toHaveLength(2)
    expect((sent[1]!.body as { lock_out: boolean; rules: unknown[] }).lock_out).toBe(true)
    expect((sent[1]!.body as { rules: unknown[] }).rules).toHaveLength(1)
  })

  it('says why rules were not taken', async () => {
    on('PUT', '/api/v1/security/firewall', problem(502, 'firewall_failed', 'nft: syntax error'))
    await mountApp(FirewallTab)
    await press(t('firewall.edit'))
    await type('#firewall-rule-interface', 'enp1s0')
    await submit('#firewall-rule')
    await click(t('common.apply'))
    expect(page()).toContain('nftables did not take the rules; nothing changed (nft: syntax error)')
  })

  it('counts down a change waiting and keeps it', async () => {
    on(
      'GET',
      '/api/v1/security/firewall',
      setup({
        pending: { settings: settings({ fallback: 'deny' }), revert_at: Date.now() / 1000 + 60 },
      }),
    )
    on('POST', '/api/v1/security/firewall/confirm', null)
    await mountApp(FirewallTab)
    expect(page()).toContain(t('firewall.pending.title'))
    expect(page()).toMatch(/go back by themselves in (59|60) seconds/)
    expect(labelled(t('firewall.moveDown')).disabled).toBe(true)
    await click(t('firewall.pending.confirm'))
    expect(called('POST', '/api/v1/security/firewall/confirm')).toHaveLength(1)
  })

  it('lets a blocked service through from the local network', async () => {
    on('GET', '/api/v1/security/firewall', setup({ blocked: ['smb', 'project:media', 'gone'] }))
    on('PUT', '/api/v1/security/firewall', setup())
    await mountApp(FirewallTab)
    expect(page()).toContain(t('firewall.blocked.title'))
    expect(page()).toContain('Docker project media')
    const allow = [...document.querySelectorAll<HTMLButtonElement>('button')].filter(
      (b) => b.textContent?.trim() === t('firewall.blocked.allow'),
    )
    expect(allow).toHaveLength(2)
    allow[0]!.click()
    await settle()
    expect(page()).toContain(
      t('firewall.blocked.allowTitle', { service: t('firewall.services.smb') }),
    )
    await submit('#firewall-allow')
    const sent = called('PUT', '/api/v1/security/firewall')[0]!.body as {
      rules: { service: string; source: string; action: string }[]
    }
    expect(sent.rules).toHaveLength(3)
    expect(sent.rules[0]).toMatchObject({
      service: 'smb',
      source: '192.168.1.0/24',
      action: 'allow',
    })
  })

  it('lets a blocked service through from anyone or one address', async () => {
    on('GET', '/api/v1/security/firewall', setup({ blocked: ['smb'], local_networks: [] }))
    on('PUT', '/api/v1/security/firewall', problem(502, 'firewall_failed'))
    await mountApp(FirewallTab)
    await click(t('firewall.blocked.allow'))
    await choose(t('firewall.rule.sourceCustom'))
    await type('input[placeholder="192.168.1.0/24"]', '10.0.0.7')
    await submit('#firewall-allow')
    const sent = called('PUT', '/api/v1/security/firewall')[0]!.body as {
      rules: { source: string }[]
    }
    expect(sent.rules[0]!.source).toBe('10.0.0.7')
    expect(page()).toContain('nftables did not take the rules')
  })

  it('switches a rule off, reorders by dragging and resets', async () => {
    await mountApp(FirewallTab)
    const [first, second] = document.querySelectorAll('tr[data-rule]')
    first!.dispatchEvent(new Event('dragstart'))
    second!.dispatchEvent(new Event('drop', { cancelable: true }))
    await settle()
    expect(rows()[0]).toContain('8000-8100')
    labelled(t('firewall.columns.enabled')).click()
    await settle()
    await click(t('common.reset'))
    expect(rows()[0]).toContain('Web interface')
    expect(document.querySelector('footer')).toBeNull()
  })
})

describe("a restored backup's firewall rules", () => {
  const restored = {
    settings: settings({ fallback: 'deny' }),
    restored_at: 1_790_000_000,
  }

  it('shows them beside the defaults and applies them to be confirmed', async () => {
    on(
      'GET',
      '/api/v1/security/firewall',
      setup({ current: settings({ enabled: false }), restored }),
    )
    on('POST', '/api/v1/security/firewall/restored/apply', setup())
    await mountApp(FirewallTab)
    const card = document.querySelector('[data-restored]')?.textContent ?? ''
    expect(card).toContain(t('firewall.restored.title'))
    expect(card).toContain(
      t('firewall.restored.on', { count: 2, fallback: t('firewall.restored.deny') }),
    )
    await click(t('firewall.restored.apply'))
    expect(called('POST', '/api/v1/security/firewall/restored/apply')[0]?.body).toEqual({
      lock_out: false,
    })
  })

  it('asks before they shut out this browser, and forgets them', async () => {
    on('GET', '/api/v1/security/firewall', setup({ restored }))
    on(
      'POST',
      '/api/v1/security/firewall/restored/apply',
      ({ body }: { body: { lock_out: boolean } }) =>
        body.lock_out ? setup() : problem(409, 'would_lock_out'),
    )
    on('DELETE', '/api/v1/security/firewall/restored', null)
    await mountApp(FirewallTab)
    await click(t('firewall.restored.apply'))
    expect(page()).toContain(t('firewall.lockOut.title'))
    await click(t('firewall.lockOut.confirm'))
    expect(called('POST', '/api/v1/security/firewall/restored/apply')[1]?.body).toEqual({
      lock_out: true,
    })
    await click(t('firewall.restored.forget'))
    expect(called('DELETE', '/api/v1/security/firewall/restored')).toHaveLength(1)
  })
})
