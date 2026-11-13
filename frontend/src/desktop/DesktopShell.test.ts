import { flushPromises } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import { live, push, sample } from '@/shared/test-support/live'
import {
  click,
  labelled,
  mountApp,
  page,
  press,
  screen,
  settle,
  type,
} from '@/desktop/testing/mount'
import { called, on, reset } from '@/shared/test-support/server'
import { i18n } from '@/shared/i18n'
import { DELAY_MS, RETRY_MS } from '@/shared/lib/star-prompt'

import DesktopShell from './DesktopShell.vue'
import { useWindows } from './windows'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

let router: Router

async function shell(permissions?: string[]) {
  router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', name: 'desktop', component: { render: () => null } },
      { path: '/login', name: 'login', component: { render: () => null } },
    ],
  })
  await router.push('/')
  return mountApp(DesktopShell, { permissions, global: { plugins: [router] } })
}

/** The launcher's panel, while it is open. */
const launcher = () =>
  document.querySelector(`[role="dialog"][aria-label="${t('desktop.launcher')}"]`)

/** Opens a reka menu or popover the way a pointer does. */
async function pointer(element: Element): Promise<void> {
  element.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }))
  element.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, button: 0 }))
  element.dispatchEvent(new MouseEvent('click', { bubbles: true, button: 0 }))
  await settle()
}

beforeEach(() => {
  reset()
  screen.compact = false
  localStorage.clear()
  sessionStorage.clear()
  on('GET', '/api/v1/apps/docker', {
    installed: true,
    running: true,
    volume: { pool: 'tank', number: 1 },
    volumes: [],
  })
  on('GET', '/api/v1/jobs', [])
  on('GET', '/api/v1/notifications', {
    notifications: [
      {
        id: 1,
        level: 'warning',
        code: 'pool_degraded',
        params: { pool: 'cold' },
        at: 1_790_000_000,
      },
    ],
    read_up_to: 0,
  })
  on('GET', '/api/v1/preferences', {
    theme: 'system',
    language: 'en',
    widgets: null,
    desktop_icons: null,
    wallpaper: null,
  })
  on('PATCH', '/api/v1/preferences', (call: { body: unknown }) => call.body)
  on('GET', '/api/v1/system/info', {
    hostname: 'nas',
    kernel_release: '6.12',
    architecture: 'x86_64',
    cpu_threads: 4,
    memory_total_bytes: 8e9,
    swap_total_bytes: 0,
    uefi: false,
    version: '0.1.1',
    uptime_seconds: 60,
  })
  on('GET', '/api/v1/storage/pools', [
    { name: 'tank', state: 'online', used_bytes: 1e12, available_bytes: 3e12 },
  ])
  on('GET', '/api/v1/storage/overview', {
    pools: 1,
    disks: 2,
    used_bytes: 1e12,
    available_bytes: 3e12,
    verdict: null,
    concerns: [],
  })
  on('GET', '/api/v1/system/metrics/history', { at: [] })
  on('POST', '/api/v1/auth/logout', null)
})

afterEach(() => {
  screen.compact = false
})

describe('the desktop', () => {
  it('connects, shows the apps this account may use and the widgets', async () => {
    await shell()
    expect(live.connected).toBe(true)
    expect(page()).toContain(t('apps.files'))
    expect(called('GET', '/api/v1/notifications').length).toBeGreaterThan(0)
    push('metrics', sample(1))
    await settle()
    expect(page()).toContain('23')
  })

  it('lists Virtual machines once the plugin is set up, and never its console', async () => {
    const plugin = {
      hardware: {
        architecture: 'amd64',
        architecture_supported: true,
        kvm: true,
        cpu_virtualization: 'vmx',
        iommu: false,
        swtpm: true,
        secure_boot: true,
      },
      installed: true,
      storage: null,
      pools: [],
      cores: 4,
      memory_bytes: 8e9,
      bridges: [],
    }
    on('GET', '/api/v1/vms/plugin', plugin)
    await shell()
    expect(page()).not.toContain(t('apps.vms'))
    reset()
    on('GET', '/api/v1/vms/plugin', { ...plugin, storage: { pool: 'tank', volume: 1 } })
    await shell()
    expect(page()).toContain(t('apps.vms'))
    expect(page()).not.toContain(t('apps.vm-console'))
    // Without the permission it is not asked about at all.
    reset()
    await shell(['account.self', 'files.read'])
    expect(called('GET', '/api/v1/vms/plugin')).toHaveLength(0)
  })

  it('opens apps from the launcher and the icons, and manages their windows', async () => {
    await shell()
    await press(t('desktop.launcher'))
    // The resource monitor is a page of the control panel now.
    await type(`input[aria-label="${t('desktop.search')}"]`, t('controlPanel.monitor'))
    document
      .querySelector<HTMLInputElement>(`input[aria-label="${t('desktop.search')}"]`)!
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    await settle()
    expect(
      document.querySelector(`section[role="dialog"][aria-label="${t('apps.settings')}"]`),
    ).not.toBeNull()
    expect(
      useWindows().state.windows.map((window) => [window.appId, window.section]),
    ).toContainEqual(['settings', 'monitor'])
    await press(t('desktop.maximize'))
    await press(t('desktop.restore'))
    await press(t('desktop.minimize'))
    await press(t('desktop.showDesktop'))
    await press(t('desktop.close'))

    const icons = document.querySelectorAll<HTMLElement>(
      `[aria-label="${t('desktop.apps')}"] button`,
    )
    icons[0]?.click()
    await settle()
    await press(t('desktop.launcher'))
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    await settle()
    expect(launcher()).toBeNull()
  })

  it('finds a page by what is set on it, and opens the result the arrow keys choose', async () => {
    // jsdom does not scroll; the launcher keeps the chosen result in view.
    const scrolled = vi.fn()
    Element.prototype.scrollIntoView = scrolled
    await shell()
    await press(t('desktop.launcher'))
    const field = `input[aria-label="${t('desktop.search')}"]`
    const key = async (name: string) => {
      document
        .querySelector(field)!
        .dispatchEvent(new KeyboardEvent('keydown', { key: name, cancelable: true }))
      await settle()
    }
    const results = () =>
      [...launcher()!.querySelectorAll('button')].map((item) => item.textContent?.trim())

    // Nothing typed: the apps, and no arrow key chooses among them.
    expect(results()).toContain(t('apps.files'))
    await key('ArrowDown')
    expect(launcher()!.querySelector('[data-selected]')).toBeNull()

    // SSH is set on the Terminal page, which does not say so in its name.
    await type(field, 'ssh')
    expect(results().some((text) => text?.startsWith(t('controlPanel.terminal')))).toBe(true)
    await type(field, 'no such thing')
    expect(launcher()!.textContent).toContain(t('common.noMatch'))
    await key('ArrowDown')

    // Both storage pages; down, down wraps to the first, up to the last.
    await type(field, t('controlPanel.groupStorage'))
    const chosen = () => launcher()!.querySelector('[data-selected]')?.textContent
    const first = chosen()
    await key('ArrowDown')
    const second = chosen()
    expect(second).not.toBe(first)
    await key('ArrowUp')
    expect(chosen()).toBe(first)
    await key('ArrowUp')
    await key('Enter')
    expect(launcher()).toBeNull()
    const opened = useWindows().state.windows.find((window) => window.appId === 'settings')
    expect(opened?.section).toBeDefined()
    expect(scrolled).toHaveBeenCalled()
  })

  it('closes the launcher on a click elsewhere, and by its own switch', async () => {
    await shell()
    const down = (element: Element | Document) =>
      element.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    await press(t('desktop.launcher'))
    // Inside it, a click is the launcher's own.
    down(launcher()!.querySelector('input')!)
    await settle()
    expect(launcher()).not.toBeNull()
    // Its switch in the bar closes it itself: once, not twice.
    down(labelled(t('desktop.launcher')))
    await press(t('desktop.launcher'))
    expect(launcher()).toBeNull()
    await press(t('desktop.launcher'))
    down(document)
    await settle()
    expect(launcher()).not.toBeNull()
    down(document.querySelector(`[aria-label="${t('desktop.desktop')}"]`)!)
    await settle()
    expect(launcher()).toBeNull()
  })

  it('lets the widgets make way for a window that reaches into their column', async () => {
    await shell()
    const widgets = () =>
      document.querySelector<HTMLElement>(`aside[aria-label="${t('desktop.widgets')}"]`)!
    expect(widgets().style.display).toBe('')
    // Wider than the desktop here, so it reaches the right edge.
    document.querySelectorAll<HTMLElement>(`[aria-label="${t('desktop.apps')}"] button`)[0]?.click()
    await settle()
    expect(widgets().style.display).toBe('none')
    await press(t('desktop.showDesktop'))
    expect(widgets().style.display).toBe('')
  })

  it('hides and shows the widgets, remembered in this browser', async () => {
    await shell()
    await press(t('desktop.widgets'))
    expect(localStorage.getItem('zendiloos.widgets.open')).toBe('false')
    await press(t('desktop.widgets'))
    expect(localStorage.getItem('zendiloos.widgets.open')).toBe('true')
  })

  it('shows the notifications and clears them', async () => {
    on('POST', '/api/v1/notifications/read', null)
    on('POST', '/api/v1/notifications/clear', null)
    await shell()
    await pointer(document.querySelector(`button[aria-label="${t('desktop.notifications')}"]`)!)
    await click(new RegExp(t('desktop.clearNotifications')))
    expect(called('POST', '/api/v1/notifications/clear')).toHaveLength(1)
  })

  it('signs out through the account menu', async () => {
    const { wrapper } = await shell()
    wrapper.findComponent({ name: 'UserMenu' }).vm.$emit('signOut')
    await settle()
    await flushPromises()
    expect(called('POST', '/api/v1/auth/logout')).toHaveLength(1)
    expect(router.currentRoute.value.name).toBe('login')
  })

  it('opens the account menu with what an administrator may do', async () => {
    on('POST', '/api/v1/system/power', null)
    await shell()
    await pointer(document.querySelector(`button[aria-label="${t('desktop.account')}"]`)!)
    expect(page()).toContain(t('desktop.restart'))
    const restart = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((item) =>
      item.textContent?.includes(t('desktop.restart')),
    )
    restart?.click()
    await settle()
  })

  it('changes the own password from the account menu', async () => {
    await shell()
    await pointer(document.querySelector(`button[aria-label="${t('desktop.account')}"]`)!)
    const change = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((item) =>
      item.textContent?.includes(t('settings.changePassword')),
    )!
    change.click()
    await settle()
    const dialog = document.querySelector('[role="dialog"]')!
    expect(dialog.textContent).toContain(t('myAccount.passwordHint'))
    expect(dialog.querySelectorAll('input[type="password"]').length).toBeGreaterThan(0)
  })

  it('arranges the icons and the widgets', async () => {
    await shell()
    const icons = [...document.querySelectorAll<HTMLElement>('[draggable="true"]')]
    icons[0]!.dispatchEvent(new Event('dragstart', { bubbles: true }))
    icons[1]!.dispatchEvent(new Event('dragover', { bubbles: true, cancelable: true }))
    icons[1]!.dispatchEvent(new Event('dragleave', { bubbles: true }))
    icons[1]!.dispatchEvent(new Event('dragover', { bubbles: true, cancelable: true }))
    icons[1]!.dispatchEvent(new Event('drop', { bubbles: true, cancelable: true }))
    await settle()
    icons[0]!.dispatchEvent(new Event('dragend', { bubbles: true }))
    expect(
      called('PATCH', '/api/v1/preferences').some(
        (call) => 'desktop_icons' in (call.body as object),
      ),
    ).toBe(true)

    await click(t('desktop.editWidgets'))
    await press(t('desktop.moveDown'))
    await press(t('desktop.moveUp'))
    const box = document.querySelector<HTMLInputElement>('input[type="checkbox"]')!
    box.checked = !box.checked
    box.dispatchEvent(new Event('change'))
    await settle()
    await click(t('desktop.done'))
    expect(
      called('PATCH', '/api/v1/preferences').some((call) => 'widgets' in (call.body as object)),
    ).toBe(true)
  })

  it('shuts down from the account menu once confirmed', async () => {
    on('POST', '/api/v1/system/power', null)
    const { wrapper } = await shell()
    await pointer(document.querySelector(`button[aria-label="${t('desktop.account')}"]`)!)
    const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((entry) =>
      entry.textContent?.includes(t('desktop.shutDown')),
    )!
    item.click()
    await settle()
    wrapper
      .findComponent({ name: 'UserMenu' })
      .findComponent({ name: 'ConfirmDialog' })
      .vm.$emit('confirm')
    await settle()
    expect(called('POST', '/api/v1/system/power')[0]?.body).toEqual({ action: 'power_off' })
  })

  it('is a home screen on a phone', async () => {
    screen.compact = true
    await shell(['files.read', 'files.write'])
    expect(page()).toContain(t('apps.files'))
    expect(page()).not.toContain(t('apps.docker'))
  })

  it('asks an administrator for a star where the toasts appear, once all is calm', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      on('GET', '/api/v1/notifications', { notifications: [], read_up_to: 0 })
      await shell()
      await vi.advanceTimersByTimeAsync(DELAY_MS + RETRY_MS)
      await settle()
      const card = document.querySelector(`section[aria-label="${t('desktop.starPrompt.label')}"]`)
      expect(card?.closest('[aria-live="polite"]')).not.toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })
})
