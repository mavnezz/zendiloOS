import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { VirtualMachine, VmPlugin, VmStats } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { formatBytes, formatPercent, formatRate } from '@/shared/lib/format'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { listened, push } from '@/shared/test-support/live'
import { button, click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import VmsApp from './VmsApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string, values: Record<string, unknown> | number = {}) =>
  typeof values === 'number' ? i18n.global.t(key, values) : i18n.global.t(key, values)

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  locations: [{ pool: 'tank', volume: 1 }],
  pools: [
    { pool: 'tank', volume: 1, health: 'online', used_bytes: 1 << 30, available_bytes: 99 << 30 },
  ],
  cores: 4,
  memory_bytes: 8 * 1024 ** 3,
  bridges: ['br0'],
  internal_networks: ['vmnet0'],
  cards: ['enp1s0', 'bond0'],
  image_upload_max_bytes: 32 * 1024 ** 3,
}

function vm(id: string, name: string, extra: Partial<VirtualMachine> = {}): VirtualMachine {
  return {
    id,
    name,
    os: 'linux',
    cpus: 2,
    memory_mib: 2048,
    firmware: 'uefi',
    disks: [{ slot: 0, size_gib: 20, bus: 'virtio', pool: 'tank', volume: 1 }],
    nics: [{ slot: 0, model: 'virtio', network: { kind: 'user' }, mac: '52:54:00:00:00:01' }],
    drives: [{ slot: 0, image: null }],
    boot: [
      { kind: 'drive', slot: 0 },
      { kind: 'disk', slot: 0 },
    ],
    autostart: false,
    created_at: 1_790_000_000,
    state: 'shut_off',
    ...extra,
  }
}

const STATS: VmStats = {
  at: 1_790_000_100,
  vms: [
    {
      id: '0a1b2c3d4e5f',
      cpu_percent: 37.5,
      memory_used_bytes: 512 * 1024 ** 2,
      memory_bytes: 2048 * 1024 ** 2,
      received_bytes_per_second: 2048,
      sent_bytes_per_second: 1024,
    },
  ],
  running: 1,
  total: 2,
  cpu_percent: 18.75,
  memory_assigned_bytes: 2 * 1024 ** 3,
  host_memory_bytes: 8 * 1024 ** 3,
  host_cores: 4,
}

const openApp = vi.fn()
const section = ref<string | undefined>(undefined)

async function mountVms() {
  return mountApp(VmsApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: {
          section,
          navigate: (next: string) => (section.value = next),
          openApp,
        },
      },
    },
  })
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

function card(id: string): string {
  return document.querySelector(`[data-vm="${id}"]`)?.textContent ?? ''
}

function within(id: string, text: string): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>(`[data-vm="${id}"] button`)].find(
    (candidate) => candidate.textContent?.trim() === text,
  )!
}

beforeEach(() => {
  reset()
  openApp.mockClear()
  section.value = undefined
  on('GET', '/api/v1/vms/plugin', PLUGIN)
  on('GET', '/api/v1/vms/images', [])
  on('GET', '/api/v1/vms', [
    vm('0a1b2c3d4e5f', 'Debian', { state: 'running', autostart: true }),
    vm('00000000000b', 'Büro-PC', {
      os: 'windows',
      cpus: 1,
      memory_mib: 1536,
      disks: [
        { slot: 0, size_gib: 64, bus: 'sata', pool: 'tank', volume: 1 },
        { slot: 1, size_gib: 100, bus: 'sata', pool: 'tank', volume: 1 },
      ],
      nics: [
        {
          slot: 0,
          model: 'e1000e',
          network: { kind: 'bridge', bridge: 'br0' },
          mac: '52:54:00:00:00:02',
        },
      ],
    }),
  ])
})

describe('the Virtual machines app', () => {
  it('points to the App Center until the plugin is set up', async () => {
    on('GET', '/api/v1/vms/plugin', { ...PLUGIN, storage: null })
    await mountVms()
    expect(page()).toContain(t('vms.notReady'))
    expect(called('GET', '/api/v1/vms')).toHaveLength(0)
    expect(document.querySelector('nav')).toBeNull()
    await click(t('vms.toAppCenter'))
    expect(openApp).toHaveBeenCalledWith('apps', 'vms')
  })

  it('offers to make the first VM, or to fill the library first', async () => {
    on('GET', '/api/v1/vms', [])
    await mountVms()
    expect(page()).toContain(t('vms.empty'))
    await click(t('vms.devices.toImages'))
    expect(section.value).toBe('images')
    expect(page()).toContain(t('vms.images.empty'))
    section.value = 'machines'
    await settle()
    await click(t('vms.create'))
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(t('vms.wizard.title'))
  })

  it('has machines and images in its sidebar, with how many of each', async () => {
    await mountVms()
    const nav = document.querySelector('nav')?.textContent ?? ''
    expect(nav).toContain(t('vms.sections.machines'))
    expect(nav).toContain(t('vms.sections.images'))
    expect(nav).toMatch(new RegExp(`${t('vms.sections.machines')}\\s*2`))
  })

  it('has the storage, the networks and the VMs’ log as pages of their own', async () => {
    on('GET', '/api/v1/vms/locations', [])
    on('GET', '/api/v1/logs/audit', {
      entries: [
        {
          id: 9,
          at: 1_790_000_000,
          actor: 'alice',
          client_ip: null,
          action: 'vms.location_add',
          target: 'cold',
          outcome: 'success',
          reason: null,
          request_id: null,
        },
      ],
      older: null,
    })
    await mountVms()
    const nav = document.querySelector('nav')?.textContent ?? ''
    for (const name of ['storage', 'networks', 'log'])
      expect(nav).toContain(t(`vms.sections.${name}`))
    section.value = 'storage'
    await settle()
    expect(page()).toContain(t('vms.storage.description'))
    section.value = 'networks'
    await settle()
    expect(page()).toContain(t('vms.networks.kinds.internal'))
    expect(page()).toContain('Büro-PC')
    section.value = 'log'
    await settle()
    expect(called('GET', '/api/v1/logs/audit').at(-1)?.query).toMatchObject({ prefix: 'vms.' })
    expect(page()).toContain(t('vms.log.actions.location_add'))
    expect(page()).toContain('cold')
  })

  it('shows each VM with its state, resources and network', async () => {
    await mountVms()
    const office = card('00000000000b')
    expect(office).toContain(t('vms.os.windows'))
    expect(office).toContain(t('vms.states.shut_off'))
    expect(office).toContain('1.5 GiB')
    expect(office).toContain(t('vms.disks', 2))
    expect(office).toContain('164 GiB')
    expect(office).toContain(t('vms.networks.on.bridge', { device: 'br0' }))
    const debian = card('0a1b2c3d4e5f')
    expect(debian).toContain(t('vms.states.running'))
    expect(debian).toContain(t('vms.userNetwork'))
    expect(debian).toContain(t('vms.autostart'))
    expect(debian).toContain(t('vms.load.waiting'))
  })

  it('shows what the VMs use as it comes, and stops listening when it closes', async () => {
    const { wrapper } = await mountVms()
    expect(listened('vm_stats')).toBe(true)
    push('vm_stats', STATS)
    await settle()
    const overview = document.querySelector('[data-testid="vms-overview"]')?.textContent ?? ''
    expect(overview).toContain(t('vms.overview.runningValue', { running: 1, total: 2 }))
    expect(overview).toContain(formatPercent(18.75))
    const gib = 1024 ** 3
    expect(overview).toContain(
      t('vms.overview.memoryValue', {
        assigned: formatBytes(2 * gib),
        total: formatBytes(8 * gib),
      }),
    )
    const debian = card('0a1b2c3d4e5f')
    expect(debian).toContain(formatPercent(37.5))
    expect(debian).toContain(
      t('vms.overview.memoryValue', { assigned: formatBytes(512 * 1024 ** 2), total: '2 GiB' }),
    )
    expect(debian).toContain(`↓ ${formatRate(2048)}`)
    expect(debian).toContain(`↑ ${formatRate(1024)}`)
    const meters = document.querySelectorAll('[data-vm="0a1b2c3d4e5f"] [role="meter"]')
    expect([...meters].map((meter) => meter.getAttribute('aria-valuenow'))).toEqual(['38', '25'])
    // A card through passt has nothing the NAS counts.
    push('vm_stats', {
      ...STATS,
      vms: [{ ...STATS.vms[0]!, received_bytes_per_second: null, sent_bytes_per_second: null }],
    })
    await settle()
    expect(card('0a1b2c3d4e5f')).toContain(t('vms.load.notCounted'))
    wrapper.unmount()
    expect(listened('vm_stats')).toBe(false)
  })

  it('starts a VM, opens its console, and shuts it down from its split button', async () => {
    on('POST', '/api/v1/vms/00000000000b/start', vm('00000000000b', 'Büro-PC'))
    on('POST', '/api/v1/vms/0a1b2c3d4e5f/shutdown', vm('0a1b2c3d4e5f', 'Debian'))
    on('POST', '/api/v1/vms/0a1b2c3d4e5f/restart', vm('0a1b2c3d4e5f', 'Debian'))
    await mountVms()
    expect(within('00000000000b', t('vms.console')).disabled).toBe(true)
    within('00000000000b', t('vms.start')).click()
    await settle()
    expect(called('POST', '/api/v1/vms/00000000000b/start')).toHaveLength(1)
    within('0a1b2c3d4e5f', t('vms.console')).click()
    expect(openApp).toHaveBeenCalledWith('vm-console', '0a1b2c3d4e5f')
    within('0a1b2c3d4e5f', t('vms.shutDown')).click()
    await settle()
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/shutdown')).toHaveLength(1)
    await menu(t('vms.shutDownMenu', { name: 'Debian' }), t('vms.restart'))
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/restart')).toHaveLength(1)
  })

  it('says why a change was refused', async () => {
    on(
      'POST',
      '/api/v1/vms/00000000000b/start',
      problem(409, 'refused', 'domain is already active'),
    )
    await mountVms()
    await click(t('vms.start'))
    expect(page()).toContain('domain is already active')
  })

  it('forces a VM off only once asked, and turns autostart off', async () => {
    on('POST', '/api/v1/vms/0a1b2c3d4e5f/force-off', vm('0a1b2c3d4e5f', 'Debian'))
    on('PUT', '/api/v1/vms/0a1b2c3d4e5f/autostart', vm('0a1b2c3d4e5f', 'Debian'))
    await mountVms()
    await menu(t('vms.shutDownMenu', { name: 'Debian' }), t('vms.forceOff'))
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/force-off')).toHaveLength(0)
    expect(page()).toContain(t('vms.forceOffText'))
    await click(t('vms.forceOff'))
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/force-off')).toHaveLength(1)

    await menu(t('vms.more', { name: 'Debian' }), t('vms.autostartOff'))
    expect(called('PUT', '/api/v1/vms/0a1b2c3d4e5f/autostart')[0]?.body).toEqual({
      autostart: false,
    })
  })

  it('opens a VM to edit it', async () => {
    await mountVms()
    await menu(t('vms.more', { name: 'Büro-PC' }), t('vms.editVm'))
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
      t('vms.edit.title', { name: 'Büro-PC' }),
    )
  })

  it('deletes a VM only with its name typed again', async () => {
    on('POST', '/api/v1/vms/00000000000b/delete', null)
    await mountVms()
    await menu(t('vms.more', { name: 'Büro-PC' }), t('vms.delete'))
    const submit = button(t('vms.deleteDialog.submit'))
    expect(submit.disabled).toBe(true)
    await type('#vm-delete-confirm', 'büro-pc')
    expect(submit.disabled).toBe(true)
    await type('#vm-delete-confirm', 'Büro-PC')
    expect(submit.disabled).toBe(false)
    submit.click()
    await settle()
    expect(called('POST', '/api/v1/vms/00000000000b/delete')[0]?.body).toEqual({
      confirm: 'Büro-PC',
    })
  })
})
