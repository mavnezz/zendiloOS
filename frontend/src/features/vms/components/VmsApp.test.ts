import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { VirtualMachine, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { button, click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import VmsApp from './VmsApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> | number = {}) =>
  typeof values === 'number' ? i18n.global.t(key, values) : i18n.global.t(key, values)

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  pools: [{ pool: 'tank', volume: 1 }],
  cores: 2,
  memory_bytes: 2 * 1024 ** 3,
  bridges: ['br0'],
}

function vm(id: string, name: string, extra: Partial<VirtualMachine> = {}): VirtualMachine {
  return {
    id,
    name,
    os: 'linux',
    cpus: 2,
    memory_mib: 2048,
    firmware: 'uefi',
    disk_gib: 20,
    network: { kind: 'user' },
    autostart: false,
    iso: null,
    pool: 'tank',
    volume: 1,
    created_at: 1_790_000_000,
    state: 'shut_off',
    ...extra,
  }
}

const openApp = vi.fn()

async function mountVms() {
  return mountApp(VmsApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: { section: ref(undefined), navigate: vi.fn(), openApp },
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

beforeEach(() => {
  reset()
  openApp.mockClear()
  on('GET', '/api/v1/vms/plugin', PLUGIN)
  on('GET', '/api/v1/vms', [
    vm('0a1b2c3d4e5f', 'Debian', { state: 'running', autostart: true }),
    vm('00000000000b', 'Büro-PC', {
      os: 'windows',
      cpus: 1,
      memory_mib: 1536,
      disk_gib: 64,
      network: { kind: 'bridge', bridge: 'br0' },
    }),
  ])
})

describe('the Virtual machines app', () => {
  it('points to the App Center until the plugin is set up', async () => {
    on('GET', '/api/v1/vms/plugin', { ...PLUGIN, storage: null })
    await mountVms()
    expect(page()).toContain(t('vms.notReady'))
    expect(called('GET', '/api/v1/vms')).toHaveLength(0)
    await click(t('vms.toAppCenter'))
    expect(openApp).toHaveBeenCalledWith('apps', 'vms')
  })

  it('offers to make the first VM', async () => {
    on('GET', '/api/v1/vms', [])
    await mountVms()
    expect(page()).toContain(t('vms.empty'))
    await click(t('vms.create'))
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(t('vms.wizard.title'))
  })

  it('shows each VM with its state, resources and network', async () => {
    await mountVms()
    const debian = card('0a1b2c3d4e5f')
    expect(debian).toContain('Debian')
    expect(debian).toContain(t('vms.states.running'))
    expect(debian).toContain(t('vms.cpus', 2))
    expect(debian).toContain('2 GiB')
    expect(debian).toContain('20 GiB')
    expect(debian).toContain(t('vms.userNetwork'))
    expect(debian).toContain(t('vms.autostart'))
    const office = card('00000000000b')
    expect(office).toContain(t('vms.os.windows'))
    expect(office).toContain(t('vms.states.shut_off'))
    expect(office).toContain('1.5 GiB')
    expect(office).toContain(t('vms.bridgeNetwork', { bridge: 'br0' }))
  })

  it('starts, shuts down and opens the console of a VM', async () => {
    on('POST', '/api/v1/vms/00000000000b/start', vm('00000000000b', 'Büro-PC'))
    on('POST', '/api/v1/vms/0a1b2c3d4e5f/shutdown', vm('0a1b2c3d4e5f', 'Debian'))
    await mountVms()
    const within = (id: string, text: string) =>
      [...document.querySelectorAll<HTMLButtonElement>(`[data-vm="${id}"] button`)].find(
        (candidate) => candidate.textContent?.trim() === text,
      )!
    expect(within('00000000000b', t('vms.console')).disabled).toBe(true)
    within('00000000000b', t('vms.start')).click()
    await settle()
    expect(called('POST', '/api/v1/vms/00000000000b/start')).toHaveLength(1)
    within('0a1b2c3d4e5f', t('vms.shutDown')).click()
    await settle()
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/shutdown')).toHaveLength(1)
    within('0a1b2c3d4e5f', t('vms.console')).click()
    expect(openApp).toHaveBeenCalledWith('vm-console', '0a1b2c3d4e5f')
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
    await menu(t('vms.more', { name: 'Debian' }), t('vms.forceOff'))
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/force-off')).toHaveLength(0)
    expect(page()).toContain(t('vms.forceOffText'))
    await click(t('vms.forceOff'))
    expect(called('POST', '/api/v1/vms/0a1b2c3d4e5f/force-off')).toHaveLength(1)

    await menu(t('vms.more', { name: 'Debian' }), t('vms.autostartOff'))
    expect(called('PUT', '/api/v1/vms/0a1b2c3d4e5f/autostart')[0]?.body).toEqual({
      autostart: false,
    })
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
