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
import type { DevicePlan, NetworkInfo, NetworkSetup } from '@/shared/api/types'
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

const managed: NetworkSetup = {
  managed: true,
  manager: 'networkd',
  pending: null,
  takeover: null,
  devices: [],
  internal: [],
}

/** What the owner's NAS shows: ifupdown with DHCP on its first port. */
const ifupdown: NetworkSetup = {
  managed: false,
  manager: 'ifupdown',
  pending: null,
  devices: [],
  internal: [],
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

/** Opens a reka menu the way a pointer does, and picks `item`. */
async function menu(label: string, item: string): Promise<void> {
  const trigger = button(label)
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

/** Ticks a member in the dialog. */
function check(name: string): void {
  const label = [...document.querySelectorAll('[role="dialog"] fieldset label')].find((l) =>
    l.textContent?.includes(name),
  )
  const box = label?.querySelector('input')
  if (!box) throw new Error(`no member ${name}`)
  box.checked = true
  box.dispatchEvent(new Event('change'))
}

/** Waits for the plan the dialog asks for a moment after a change. */
async function planned(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 350))
  await settle()
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
    // No button switches a card off at once: applying asks first.
    expect(document.body.textContent).not.toContain(t('network.change.switchOff'))
    await submit('[role="dialog"] form')
    expect(page()).toContain(t('network.change.offQuestion', { name: 'eth1' }))
    expect(called('PUT', '/api/v1/network/interfaces/eth1')).toHaveLength(0)
    // Going back asks again next time.
    await click(t('desktop.back'))
    expect(page()).not.toContain(t('network.change.offQuestion', { name: 'eth1' }))
    await submit('[role="dialog"] form')
    expect(called('PUT', '/api/v1/network/interfaces/eth1')).toHaveLength(0)
    await click(t('network.change.switchOff'))
    expect(called('PUT', '/api/v1/network/interfaces/eth1')[0]?.body).toEqual({
      ipv4: { mode: 'off' },
      ipv6: 'off',
      dns: [],
    })
  })

  it('adds a bridge over a card, saying first what becomes of it', async () => {
    const plan: DevicePlan = {
      device: null,
      interfaces: [
        {
          interface: 'br0',
          becomes: {
            fate: 'settings',
            settings: { ipv4: { mode: 'dhcp', keep: '192.168.1.20' }, ipv6: 'automatic', dns: [] },
          },
        },
        {
          interface: 'eth0',
          becomes: { fate: 'member', device: 'br0', moved: true, dropped: false },
        },
      ],
    }
    on('POST', '/api/v1/network/devices/plan', plan)
    on('POST', '/api/v1/network/devices', waiting(null))
    await mountApp(NetworkSection, { props: { canManage: true } })
    await menu(t('network.add.button'), t('network.add.bridge'))
    expect(page()).toContain(t('network.device.titles.bridge'))
    expect((document.querySelector('#device-name') as HTMLInputElement).value).toBe('br0')
    // Free wired cards only: no Wi-Fi, nothing of Docker's.
    const offered = [...document.querySelectorAll('[role="dialog"] fieldset label')].map((l) =>
      l.textContent?.trim(),
    )
    expect(offered).toHaveLength(2)
    expect(page()).toContain(t('network.device.noMembersBridge'))
    check('eth0')
    await planned()
    expect(called('POST', '/api/v1/network/devices/plan').at(-1)?.body).toEqual({
      action: 'make',
      device: { kind: 'bridge', name: 'br0', members: ['eth0'], stp: false },
    })
    expect(page()).toContain(t('network.plan.moves', { device: 'br0' }))
    expect(page()).toContain(t('network.takeover.dhcpKeeps', { address: '192.168.1.20' }))
    expect(page()).toContain(t('network.device.firewallHint'))

    on(
      'GET',
      '/api/v1/network/setup',
      waiting({
        interface: 'br0',
        takeover: false,
        device: 'made',
        revert_at: Date.now() / 1000 + 120,
      }),
    )
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/network/devices')[0]?.body).toEqual({
      kind: 'bridge',
      name: 'br0',
      members: ['eth0'],
      stp: false,
    })
    expect(page()).toContain(t('network.pending.deviceTitle.made', { name: 'br0' }))
    expect(page()).toContain(t('network.pending.deviceHint', { seconds: 120 }).slice(0, 20))
  })

  it('explains a bond’s modes and says what is wrong with a choice', async () => {
    on(
      'POST',
      '/api/v1/network/devices/plan',
      problem(422, 'invalid_input', 'a bond needs at least one member'),
    )
    await mountApp(NetworkSection, { props: { canManage: true } })
    await menu(t('network.add.button'), t('network.add.bond'))
    expect((document.querySelector('#device-name') as HTMLInputElement).value).toBe('bond0')
    expect(page()).toContain(t('network.devices.modeHints.active_backup'))
    await type('#device-mode', 'lacp')
    expect(page()).toContain(t('network.devices.modeHints.lacp'))
    await planned()
    expect(page()).toContain('a bond needs at least one member')
    const make = document.querySelector<HTMLButtonElement>('[role="dialog"] button[type="submit"]')
    expect(make?.disabled).toBe(true)
    expect(called('POST', '/api/v1/network/devices/plan').at(-1)?.body).toEqual({
      action: 'make',
      device: { kind: 'bond', name: 'bond0', members: [], mode: 'lacp' },
    })
  })

  it('adds a VLAN named after its parent and id, with settings of its own', async () => {
    on('POST', '/api/v1/network/devices/plan', {
      device: null,
      interfaces: [
        { interface: 'eth0', becomes: { fate: 'parent', vlan: 'eth0.20', carries: true } },
      ],
    })
    on('POST', '/api/v1/network/devices', problem(409, 'change_pending'))
    await mountApp(NetworkSection, { props: { canManage: true } })
    await menu(t('network.add.button'), t('network.add.vlan'))
    expect((document.querySelector('#device-name') as HTMLInputElement).value).toBe('eth0.10')
    await type('#device-id', '20')
    expect((document.querySelector('#device-name') as HTMLInputElement).value).toBe('eth0.20')
    await type('#device-ipv4', 'static')
    await type('#device-address', '10.20.0.2')
    await type('#device-gateway', '10.20.0.1')
    await planned()
    expect(page()).toContain(t('network.plan.carries', { vlan: 'eth0.20' }))
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/network/devices')[0]?.body).toEqual({
      kind: 'vlan',
      name: 'eth0.20',
      parent: 'eth0',
      id: 20,
      settings: {
        ipv4: { mode: 'static', address: '10.20.0.2', prefix: 24, gateway: '10.20.0.1' },
        ipv6: 'off',
        dns: [],
      },
    })
    expect(page()).toContain(t('errors.change_pending'))
  })

  it('changes the members of a bridge made here and keeps one a VM is on', async () => {
    const bridged: NetworkInfo = {
      ...network,
      interfaces: [
        { ...network.interfaces[0]!, member_of: 'br0', addresses: [] },
        ...network.interfaces.slice(1),
        {
          name: 'br0',
          kind: 'bridge',
          state: 'up',
          mac: '52:54:00:12:34:56',
          mtu: 1500,
          member_of: null,
          addresses: [{ address: '192.168.1.20', prefix: 24, scope: 'global', dynamic: true }],
        },
      ],
    }
    const br0 = {
      name: 'br0',
      kind: 'bridge' as const,
      members: ['eth0'],
      parent: null,
      vlan_id: null,
      mode: null,
      stp: false,
      mac: '52:54:00:12:34:56',
    }
    on('GET', '/api/v1/network', bridged)
    on('GET', '/api/v1/network/setup', { ...managed, devices: [br0] })
    on('POST', '/api/v1/network/devices/plan', {
      device: br0,
      interfaces: [
        {
          interface: 'eth1',
          becomes: { fate: 'member', device: 'br0', moved: false, dropped: true },
        },
      ],
    })
    on('PUT', '/api/v1/network/devices/br0', waiting(null))
    await mountApp(NetworkSection, { props: { canManage: true } })
    expect(page()).toContain(t('network.devices.members', { members: 'eth0' }))
    expect(page()).toContain(t('network.devices.madeHere'))
    expect(page()).toContain(t('network.memberOf', { name: 'br0' }))

    await press(t('network.devices.change'))
    expect(page()).toContain(t('network.device.titles.change', { name: 'br0' }))
    check('eth1')
    await planned()
    expect(page()).toContain(t('network.plan.drops', { device: 'br0' }))
    await submit('[role="dialog"] form')
    expect(called('PUT', '/api/v1/network/devices/br0')[0]?.body).toEqual({
      members: ['eth0', 'eth1'],
      mode: null,
      stp: false,
    })

    on('POST', '/api/v1/network/devices/plan', problem(409, 'device_in_use', 'web'))
    await press(t('network.devices.remove'))
    expect(page()).toContain(`${t('errors.device_in_use')} (web)`)
    expect(button(t('network.device.remove')).disabled).toBe(true)
    await press(t('desktop.close'))

    on('POST', '/api/v1/network/devices/plan', {
      device: null,
      interfaces: [
        {
          interface: 'eth0',
          becomes: {
            fate: 'settings',
            settings: { ipv4: { mode: 'dhcp', keep: '192.168.1.20' }, ipv6: 'automatic', dns: [] },
          },
        },
        { interface: 'eth1', becomes: { fate: 'switched_off' } },
      ],
    })
    on('DELETE', '/api/v1/network/devices/br0', problem(409, 'change_pending'))
    await press(t('network.devices.remove'))
    expect(page()).toContain(t('network.plan.switchedOff'))
    await click(t('network.device.remove'))
    expect(called('DELETE', '/api/v1/network/devices/br0')).toHaveLength(1)
    expect(page()).toContain(t('errors.change_pending'))
    on('DELETE', '/api/v1/network/devices/br0', waiting(null))
    await click(t('network.device.remove'))
    expect(called('DELETE', '/api/v1/network/devices/br0')).toHaveLength(2)
  })

  it('offers no devices to those who may only look', async () => {
    await mountApp(NetworkSection, { props: { canManage: false } })
    expect(document.querySelector(`button[aria-label="${t('network.add.button')}"]`)).toBeNull()
    expect(page()).not.toContain(t('network.add.button'))
  })

  it('says when the network cannot be read', async () => {
    on('GET', '/api/v1/network', problem(502, 'host_failed', 'ip failed'))
    await mountApp(NetworkSection, { props: { canManage: false } })
    expect(page()).toContain('ip failed')
  })
})
