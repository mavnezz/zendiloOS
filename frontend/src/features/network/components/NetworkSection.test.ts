import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, press, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { NetworkInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import NetworkSection from './NetworkSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const network: NetworkInfo = {
  interfaces: [
    {
      name: 'eth0',
      kind: 'ethernet',
      state: 'up',
      mac: '52:54:00:12:34:56',
      mtu: 1500,
      member_of: null,
      addresses: [
        { address: '192.168.1.20', prefix: 24, scope: 'global', dynamic: false },
        { address: 'fe80::1', prefix: 64, scope: 'link', dynamic: false },
      ],
    },
    {
      name: 'wlan0',
      kind: 'wireless',
      state: 'no_carrier',
      mac: null,
      mtu: 1500,
      member_of: null,
      addresses: [],
    },
    {
      name: 'docker0',
      kind: 'bridge-ish',
      state: 'down',
      mac: null,
      mtu: 1500,
      member_of: null,
      addresses: [],
    },
    {
      name: 'eth1',
      kind: 'ethernet',
      state: 'up',
      mac: null,
      mtu: 9000,
      member_of: null,
      addresses: [{ address: '10.0.0.5', prefix: 8, scope: 'global', dynamic: true }],
    },
  ],
  default_routes: [{ gateway: '192.168.1.1', interface: 'eth0', metric: 100 }],
  name_servers: ['127.0.0.53', '192.168.1.1'],
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/network', network)
  on('GET', '/api/v1/network/setup', { managed: true, pending: null })
})

describe('the network', () => {
  it('lists the interfaces and their addresses', async () => {
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(page()).toContain('192.168.1.20/24')
    expect(page()).toContain('fe80::1')
    expect(page()).toContain('192.168.1.1')
    expect(page()).toContain(t('network.kinds.wireless'))
    expect(page()).toContain('bridge-ish')
  })

  it('changes an interface to a static address and confirms it', async () => {
    on('PUT', '/api/v1/network/interfaces/eth1', {
      interface: 'eth1',
      revert_at: Date.now() / 1000 + 120,
    })
    on('POST', '/api/v1/network/confirm', null)
    await mountApp(NetworkSection, { props: { canManage: true } })
    const change = document.querySelectorAll<HTMLButtonElement>(
      `button[aria-label="${t('network.change.button')}"]`,
    )
    change[1]!.click()
    await settle()
    await type('#net-mode', 'static')
    await type('#net-address', '10.0.0.9')
    await type('#net-prefix', '8')
    await type('#net-gateway', '10.0.0.1')
    await type('#net-dns', '1.1.1.1, 9.9.9.9')
    await type('#net-ipv6', 'off')
    on('GET', '/api/v1/network/setup', {
      managed: true,
      pending: { interface: 'eth1', revert_at: Date.now() / 1000 + 120 },
    })
    await submit('[role="dialog"] form')
    expect(called('PUT', '/api/v1/network/interfaces/eth1')[0]?.body).toEqual({
      ipv4: { mode: 'static', address: '10.0.0.9', prefix: 8, gateway: '10.0.0.1' },
      ipv6: 'off',
      dns: ['1.1.1.1', '9.9.9.9'],
    })
    expect(page()).toContain('10.0.0.9')

    await click(t('network.pending.confirm'))
    expect(called('POST', '/api/v1/network/confirm')).toHaveLength(1)
  })

  it('starts from DHCP and says when a change is refused', async () => {
    on(
      'PUT',
      '/api/v1/network/interfaces/eth0',
      problem(409, 'refused', 'eth0 carries the default route'),
    )
    on('POST', '/api/v1/network/confirm', problem(409, 'refused', 'nothing to confirm'))
    on('GET', '/api/v1/network/setup', {
      managed: true,
      pending: { interface: 'eth0', revert_at: Date.now() / 1000 + 60 },
    })
    await mountApp(NetworkSection, { props: { canManage: true } })
    on('GET', '/api/v1/network/setup', { managed: true, pending: null })
    await click(t('network.pending.confirm'))
    expect(page()).toContain('nothing to confirm')

    await press(t('network.change.button'))
    expect((document.querySelector('#net-dns') as HTMLInputElement).value).toBe('192.168.1.1')
    await type('#net-mode', 'dhcp')
    await submit('[role="dialog"] form')
    expect(page()).toContain('eth0 carries the default route')
    await press(t('desktop.close'))
  })

  it('offers no changes where zendiloOS does not manage the network', async () => {
    on('GET', '/api/v1/network/setup', { managed: false, pending: null })
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(document.querySelector(`button[aria-label="${t('network.change.button')}"]`)).toBeNull()
  })

  it('says when the network cannot be read', async () => {
    on('GET', '/api/v1/network', problem(502, 'host_failed', 'ip failed'))
    await mountApp(NetworkSection, { props: { canManage: false } })
    expect(page()).toContain('ip failed')
  })
})
