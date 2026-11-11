import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  button,
  click,
  mountApp,
  page,
  press,
  settle,
  submit,
  type,
} from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { NetworkInfo, NetworkSetup } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'

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

const managed: NetworkSetup = { managed: true, manager: 'networkd', pending: null, takeover: null }

/** What the owner's NAS shows: ifupdown with DHCP on its first port. */
const ifupdown: NetworkSetup = {
  managed: false,
  manager: 'ifupdown',
  pending: null,
  takeover: {
    interfaces: [
      { name: 'eth0', ipv4: { mode: 'dhcp', keep: '192.168.1.20' }, ipv6: 'automatic', dns: [] },
      {
        name: 'eth1',
        ipv4: { mode: 'static', address: '10.0.0.5', prefix: 8, gateway: '10.0.0.1' },
        ipv6: 'off',
        dns: ['10.0.0.1'],
      },
    ],
    refused: [],
    installs_resolved: true,
    ifupdown_running: true,
  },
}

function waiting(pending: NetworkSetup['pending']): NetworkSetup {
  return { ...managed, pending }
}

function takeoverJob(state: 'running' | 'succeeded' | 'failed', step: string | null = null) {
  return {
    id: 7,
    kind: 'network.take_over',
    target: 'ifupdown',
    state,
    step,
    progress: null,
    error_code: state === 'failed' ? 'refused' : null,
    error_detail: state === 'failed' ? "ifupdown's files changed since they were read" : null,
    actor: 'admin',
    created_at: 1,
    updated_at: 1,
    finished_at: state === 'running' ? null : 2,
    output: [],
  }
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/network', network)
  on('GET', '/api/v1/network/setup', managed)
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
    on(
      'GET',
      '/api/v1/network/setup',
      waiting({ interface: 'eth1', takeover: false, revert_at: Date.now() / 1000 + 120 }),
    )
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
    on(
      'GET',
      '/api/v1/network/setup',
      waiting({ interface: 'eth0', takeover: false, revert_at: Date.now() / 1000 + 60 }),
    )
    await mountApp(NetworkSection, { props: { canManage: true } })
    on('GET', '/api/v1/network/setup', managed)
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
    on('GET', '/api/v1/network/setup', { ...managed, managed: false, manager: 'network_manager' })
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(document.querySelector(`button[aria-label="${t('network.change.button')}"]`)).toBeNull()
    expect(page()).toContain(t('network.networkManager'))
    expect(page()).not.toContain(t('network.takeover.button'))

    on('GET', '/api/v1/network/setup', { ...managed, managed: false, manager: 'unknown' })
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(page()).toContain(t('network.readOnly'))
  })

  it('takes the network over from ifupdown and waits for confirmation', async () => {
    on('GET', '/api/v1/network/setup', ifupdown)
    on('POST', '/api/v1/network/takeover', takeoverJob('running'))
    on('POST', '/api/v1/network/confirm', null)
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(page()).toContain(t('network.takeover.title'))
    expect(page()).toContain(t('network.takeover.dhcpKeeps', { address: '192.168.1.20' }))
    expect(page()).toContain('10.0.0.5/8 · ' + t('network.takeover.via', { gateway: '10.0.0.1' }))
    expect(page()).toContain(t('network.takeover.dns', { servers: '10.0.0.1' }))
    expect(page()).toContain(t('network.takeover.resolved'))
    expect(document.querySelector(`button[aria-label="${t('network.change.button')}"]`)).toBeNull()

    await click(t('network.takeover.button'))
    expect(called('POST', '/api/v1/network/takeover')).toHaveLength(1)
    const jobs = useJobsStore()
    jobs.update(takeoverJob('running', 'packages'))
    await settle()
    expect(page()).toContain(t('network.takeover.installing'))
    expect(button(t('network.takeover.button')).disabled).toBe(true)
    jobs.update(takeoverJob('running', 'taking_over'))
    await settle()
    expect(page()).toContain(t('network.takeover.working'))

    on(
      'GET',
      '/api/v1/network/setup',
      waiting({ interface: '', takeover: true, revert_at: Date.now() / 1000 + 120 }),
    )
    jobs.update(takeoverJob('succeeded'))
    await settle()
    expect(page()).toContain(t('network.pending.takeoverTitle'))
    await click(t('network.pending.confirm'))
    expect(called('POST', '/api/v1/network/confirm')).toHaveLength(1)
  })

  it('names what ifupdown sets up that would not be carried over', async () => {
    const refused: NetworkSetup = {
      ...ifupdown,
      takeover: {
        ...ifupdown.takeover!,
        interfaces: [],
        refused: [{ place: 'interfaces:12', reason: 'br0: bridge-ports: a bridge' }],
      },
    }
    on('GET', '/api/v1/network/setup', refused)
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(page()).toContain('interfaces:12')
    expect(page()).toContain('br0: bridge-ports: a bridge')
    expect(button(t('network.takeover.button')).disabled).toBe(true)
  })

  it('says when ifupdown has nothing to give', async () => {
    const stopped: NetworkSetup = {
      ...ifupdown,
      takeover: { ...ifupdown.takeover!, ifupdown_running: false },
    }
    on('GET', '/api/v1/network/setup', stopped)
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(page()).toContain(t('network.takeover.notRunning'))
    expect(button(t('network.takeover.button')).disabled).toBe(true)
  })

  it('says why a takeover did not begin or failed', async () => {
    on('GET', '/api/v1/network/setup', ifupdown)
    on('POST', '/api/v1/network/takeover', problem(409, 'change_pending'))
    await mountApp(NetworkSection, { props: { canManage: true } })
    await click(t('network.takeover.button'))
    expect(page()).toContain(t('errors.change_pending'))

    on('POST', '/api/v1/network/takeover', takeoverJob('running'))
    await click(t('network.takeover.button'))
    const jobs = useJobsStore()
    jobs.update(takeoverJob('running', 'taking_over'))
    await settle()
    jobs.update(takeoverJob('failed'))
    await settle()
    expect(page()).toContain("ifupdown's files changed since they were read")
  })

  it('switches an interface on with DHCP, and another off', async () => {
    on('PUT', '/api/v1/network/interfaces/eth1', waiting(null))
    on('PUT', '/api/v1/network/interfaces/eth2', waiting(null))
    on('GET', '/api/v1/network', {
      ...network,
      interfaces: [
        ...network.interfaces,
        {
          name: 'eth2',
          kind: 'ethernet',
          state: 'down',
          mac: null,
          mtu: 1500,
          member_of: null,
          addresses: [],
        },
      ],
    })
    await mountApp(NetworkSection, { props: { canManage: true } })
    const change = () =>
      document.querySelectorAll<HTMLButtonElement>(
        `button[aria-label="${t('network.change.button')}"]`,
      )
    change()[2]!.click()
    await settle()
    expect(page()).toContain(t('network.change.titleOn', { name: 'eth2' }))
    expect(document.body.textContent).not.toContain(t('network.change.switchOff'))
    await submit('[role="dialog"] form')
    expect(called('PUT', '/api/v1/network/interfaces/eth2')[0]?.body).toEqual({
      ipv4: { mode: 'dhcp' },
      ipv6: 'automatic',
      dns: [],
    })

    change()[1]!.click()
    await settle()
    await type('#net-mode', 'off')
    await type('#net-ipv6', 'off')
    expect(page()).toContain(t('network.change.switchedOff'))
    expect(document.querySelector('#net-dns')).toBeNull()
    await click(t('network.change.switchOff'))
    expect(called('PUT', '/api/v1/network/interfaces/eth1')[0]?.body).toEqual({
      ipv4: { mode: 'off' },
      ipv6: 'off',
      dns: [],
    })
  })

  it('says when the network cannot be read', async () => {
    on('GET', '/api/v1/network', problem(502, 'host_failed', 'ip failed'))
    await mountApp(NetworkSection, { props: { canManage: false } })
    expect(page()).toContain('ip failed')
  })
})
