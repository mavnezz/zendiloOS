import { beforeEach, describe, expect, it, vi } from 'vitest'

import { mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { SystemInfo } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import DeviceInfoPage from './DeviceInfoPage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const info = {
  hostname: 'zendiloos',
  kernel_release: '6.12.2',
  architecture: 'x86_64',
  cpu_threads: 4,
  memory_total_bytes: 8e9,
  swap_total_bytes: 0,
  version: '0.1.1',
  uptime_seconds: 60,
  uefi: true,
} as SystemInfo

function renameButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>(`button[aria-label="${t('hostName.rename')}"]`)
}

function submit(): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>('form button[type="submit"]')][0]!
}

beforeEach(() => {
  reset()
  i18n.global.locale.value = 'en'
  on('GET', '/api/v1/system/info', info)
  on('GET', '/api/v1/system/hostname', { name: 'zendiloos', max_length: 15 })
})

describe('renaming the machine', () => {
  it('is for those who manage the system', async () => {
    await mountApp(DeviceInfoPage)
    expect(document.querySelector('[data-testid="host-name"]')?.textContent).toBe('zendiloos')
    expect(renameButton()).toBeNull()
  })

  it('says what is wrong with a name before it is sent', async () => {
    await mountApp(DeviceInfoPage, { props: { canManage: true } })
    renameButton()!.click()
    await settle()
    expect(submit().disabled).toBe(true)

    await type('#host-name', 'nas.local')
    expect(document.querySelector('[data-testid="name-problem"]')?.textContent?.trim()).toBe(
      t('hostName.problems.characters'),
    )
    expect(submit().disabled).toBe(true)
    await type('#host-name', 'a-very-long-name')
    expect(page()).toContain(t('hostName.problems.tooLong', { max: 15 }))
    expect(called('PUT', '/api/v1/system/hostname')).toHaveLength(0)
  })

  it('renames, and tells of the certificate the browser meets', async () => {
    on('PUT', '/api/v1/system/hostname', {
      name: 'office',
      netbios_name: 'OFFICE',
      certificate_renewed: true,
      own_certificate_served: true,
    })
    await mountApp(DeviceInfoPage, { props: { canManage: true } })
    renameButton()!.click()
    await settle()

    await type('#host-name', ' Office ')
    expect(page()).toContain(t('hostName.reachedAs', { name: 'office' }))
    expect(page()).toContain(t('hostName.smbAs', { name: 'OFFICE' }))
    submit().click()
    await settle()

    expect(called('PUT', '/api/v1/system/hostname')[0]!.body).toEqual({ name: 'office' })
    expect(page()).toContain(t('hostName.done', { name: 'office', smb: 'OFFICE' }))
    expect(page()).toContain(t('hostName.certificateRenewed'))
    expect(document.querySelector('[data-testid="host-name"]')?.textContent).toBe('office')
  })

  it('tells when the certificate still names the old name, and what the server refused', async () => {
    on('PUT', '/api/v1/system/hostname', {
      name: 'office',
      netbios_name: 'OFFICE',
      certificate_renewed: false,
      own_certificate_served: false,
    })
    await mountApp(DeviceInfoPage, { props: { canManage: true } })
    renameButton()!.click()
    await settle()
    await type('#host-name', 'office')
    submit().click()
    await settle()
    expect(page()).toContain(t('hostName.certificateKept'))

    on('PUT', '/api/v1/system/hostname', problem(422, 'invalid_input', 'the name is empty'))
    document.querySelector<HTMLButtonElement>('[data-testid="renamed"] ~ div button')!.click()
    await settle()
    renameButton()!.click()
    await settle()
    await type('#host-name', 'other')
    submit().click()
    await settle()
    expect(page()).toContain('the name is empty')
  })
})
