import { ref } from 'vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import {
  button,
  click,
  labelled,
  mountApp,
  page,
  settle,
  submit,
  type,
} from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import NetworksSection from './NetworksSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const PLUGIN = {
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [{ pool: 'tank', volume: 1 }],
  pools: [],
  bridges: ['br0'],
  internal_networks: ['vmnet0', 'lab'],
  cards: ['enp1s0', 'bond0'],
} as unknown as VmPlugin

function vm(
  id: string,
  name: string,
  networks: VirtualMachine['nics'][number]['network'][],
): VirtualMachine {
  return {
    id,
    name,
    nics: networks.map((network, slot) => ({
      slot,
      model: 'virtio',
      network,
      mac: `52:54:00:00:00:0${slot}`,
    })),
  } as unknown as VirtualMachine
}

const VMS = [
  vm('000000000001', 'web', [{ kind: 'user' }, { kind: 'bridge', bridge: 'br0' }]),
  vm('000000000002', 'db', [
    { kind: 'direct', card: 'bond0' },
    { kind: 'internal', network: 'vmnet0' },
  ]),
]

const openApp = vi.fn()

async function mountNetworks(plugin = PLUGIN, vms = VMS) {
  return mountApp(NetworksSection, {
    props: { plugin, vms },
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: { section: ref(undefined), navigate: vi.fn(), openApp },
      },
    },
  })
}

function kind(name: string): string {
  return document.querySelector(`[data-kind="${name}"]`)?.textContent ?? ''
}

function network(name: string): string {
  return document.querySelector(`[data-network="${name}"]`)?.textContent ?? ''
}

beforeEach(() => {
  reset()
  openApp.mockReset()
})

describe('the networks of VMs', () => {
  it('lists every kind, what it means, and the VMs on each network', async () => {
    await mountNetworks()
    for (const name of ['user', 'bridge', 'direct', 'internal']) {
      expect(kind(name)).toContain(t(`vms.networks.kinds.${name}`))
      expect(kind(name)).toContain(t(`vms.networks.means.${name}`))
    }
    expect(kind('user')).toContain(t('vms.networks.always'))
    expect(network('user')).toContain('web')
    expect(network('br0')).toContain('web')
    expect(network('bond0')).toContain('db')
    expect(network('enp1s0')).toContain(t('vms.networks.noVms'))
    expect(network('vmnet0')).toContain('db')
    // macvtap's limit is said plainly.
    expect(
      document.querySelector('[data-testid="vm-networks-direct-warning"]')?.textContent,
    ).toContain(t('vms.networks.directWarning'))
    // A bridge is made on the Network page.
    await click(t('vms.networks.addBridge'))
    expect(openApp).toHaveBeenCalledWith('settings', 'network')
  })

  it('makes an internal network as a bridge without members, kept at once', async () => {
    on('POST', '/api/v1/network/devices', { managed: true })
    on('POST', '/api/v1/network/confirm', null)
    await mountNetworks()
    await click(new RegExp(t('vms.networks.addInternal')))
    expect((document.querySelector('#vm-internal-name') as HTMLInputElement).value).toBe('vmnet1')
    await type('#vm-internal-name', 'br0')
    expect(button(t('vms.networks.internalDialog.submit')).disabled).toBe(true)
    await type('#vm-internal-name', '-x')
    expect(button(t('vms.networks.internalDialog.submit')).disabled).toBe(true)
    await type('#vm-internal-name', 'isolated')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/network/devices')[0]?.body).toEqual({
      kind: 'bridge',
      name: 'isolated',
      members: [],
    })
    expect(called('POST', '/api/v1/network/confirm')).toHaveLength(1)
  })

  it('removes an internal network nobody is on, and says why the network refused', async () => {
    on('DELETE', '/api/v1/network/devices/lab', problem(409, 'change_pending'))
    await mountNetworks()
    expect(labelled(t('vms.networks.removeInternal', { name: 'vmnet0' })).disabled).toBe(true)
    labelled(t('vms.networks.removeInternal', { name: 'lab' })).click()
    await settle()
    await click(t('vms.networks.removeDialog.submit'))
    expect(called('DELETE', '/api/v1/network/devices/lab')).toHaveLength(1)
    expect(called('POST', '/api/v1/network/confirm')).toHaveLength(0)
    expect(page()).toContain(t('errors.change_pending'))
  })

  it('says what is missing', async () => {
    await mountNetworks({ ...PLUGIN, bridges: [], cards: [], internal_networks: [] })
    expect(kind('bridge')).toContain(t('vms.networks.noBridges'))
    // A card on a bridge gone since: listed as gone, and still none of the NAS's.
    expect(network('br0')).toContain(t('vms.networks.gone'))
    expect(kind('direct')).toContain(t('vms.networks.noCards'))
    expect(kind('internal')).toContain(t('vms.networks.noInternal'))
  })
})
