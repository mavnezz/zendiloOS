import {
  answer,
  callsTo,
  choose,
  cleanup,
  click,
  mountApp,
  on,
  page,
  problem,
  settle,
  tick,
  type,
} from '@/shared/testing/server'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { FileServices } from '@/shared/api/types'
import { useNotifications } from '@/shared/lib/notifications'
import { share } from '@/shared/testing/fixtures'

import FileServicesPage from './FileServicesPage.vue'

const services: FileServices = {
  smb: true,
  nfs: false,
  workgroup: 'WORKGROUP',
  smb_min_protocol: 'smb2_1',
  smb_encryption: 'default',
  smb_signing: 'default',
  hide_unreadable: true,
  multichannel: true,
  oplocks: true,
  leases: true,
  durable_handles: false,
  mac_compat: true,
  follow_symlinks: true,
  veto_files: '/*.tmp/',
  delete_veto_files: false,
  wsdd: true,
  bonjour: true,
  time_machine: { share_id: 3, max_bytes: 2 * 1024 ** 3 },
}

async function show(props: Record<string, unknown> = {}) {
  mountApp(FileServicesPage, { props: { canManage: true, hostname: 'nas', ...props } })
  await settle()
}

function element<T extends HTMLElement>(selector: string): T {
  return document.body.querySelector<T>(selector)!
}

beforeEach(() => {
  cleanup()
  answer('GET', '/api/v1/file-services', services)
  answer('GET', '/api/v1/shares', [share(3, 'backups')])
})

afterEach(() => vi.unstubAllGlobals())

describe('the file services, against the server', () => {
  it('turns NFS on and keeps what the server stored', async () => {
    on('PUT', '/api/v1/file-services', ({ body }) => body)
    await show()
    await click('NFS')
    element('#service-nfs').click()
    await settle()
    await click('Apply')

    expect(callsTo('PUT', '/api/v1/file-services')[0]!.body).toMatchObject({ nfs: true, smb: true })
    expect(document.body.querySelector('footer')).toBeNull()
  })

  it('shows a refusal under the page, and resets what was changed', async () => {
    on('PUT', '/api/v1/file-services', () => problem(409, 'refused', 'smbd would not start'))
    await show()
    await type('#smb-workgroup', 'office')
    await click('Apply')
    expect(page()).toContain('smbd would not start')
    await click('Reset')
    expect(element<HTMLInputElement>('#smb-workgroup').value).toBe('WORKGROUP')
  })

  it('refuses a workgroup Samba would not take', async () => {
    await show()
    await type('#smb-workgroup', '-bad name')
    expect(page()).toContain('A workgroup is 1 to 15 letters')
    expect(
      [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Apply')!
        .disabled,
    ).toBe(true)
  })

  it('copies where to connect, and says when the browser does not let it', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } })
    await show({ address: '10.0.0.5' })
    await click('\\\\nas')
    expect(writeText).toHaveBeenCalledWith('\\\\nas')
    expect(useNotifications().items.some((n) => n.title === '\\\\nas copied')).toBe(true)

    writeText.mockImplementation(() => Promise.reject(new Error('no')))
    await click('smb://10.0.0.5/')
    expect(
      useNotifications().items.some((n) => n.title === 'The browser did not allow copying'),
    ).toBe(true)
  })

  it('leads to the advanced tab, where Time Machine needs a size of a gibibyte and macOS support', async () => {
    await show()
    const link = [...document.body.querySelectorAll<HTMLButtonElement>('section button')].find(
      (b) => b.textContent?.trim() === 'Advanced',
    )!
    link.click()
    await settle()
    expect(element<HTMLSelectElement>('#time-machine-share').value).toBe('3')
    await type('input[aria-label="Size"], input[type="number"]', '0.5')
    expect(page()).toContain('A Time Machine folder holds at least 1 GiB.')
    await choose('#time-machine-share', '0')
    expect(document.body.querySelector('input[type="number"]')).toBeNull()
    element('#service-wsdd').click()
    element('#service-bonjour').click()
    await settle()
    expect(page()).toContain('Apply')
  })
})

describe('the advanced SMB options', () => {
  async function openOptions(): Promise<void> {
    await show()
    const opener = [...document.body.querySelectorAll<HTMLButtonElement>('button')].find(
      (b) =>
        b.textContent?.trim() === 'Advanced' &&
        b.getAttribute('role') !== 'tab' &&
        !b.closest('section'),
    )!
    opener.click()
    await settle()
  }

  it('sets every option in its tabs and saves them at once', async () => {
    on('PUT', '/api/v1/file-services', ({ body }) => body)
    await openOptions()
    await tick('Hide shared folders from users without permission', false)
    await choose('#smb-version', 'smb3')
    await choose('#smb-encryption', 'required')
    await tick('Durable handles')
    await tick('SMB 3 multichannel', false)
    await tick('Opportunistic locking (oplocks)', false)
    await click('macOS')
    await tick('macOS compatibility (vfs_fruit)', false)
    await click('More')
    await type('input[aria-label="Pattern"]', '/*.bak/~$*/')
    await tick('Folders holding only such files can still be deleted')
    await tick('Follow symbolic links in shared folders', false)
    await click('Save')

    expect(callsTo('PUT', '/api/v1/file-services')[0]!.body).toMatchObject({
      hide_unreadable: false,
      smb_min_protocol: 'smb3',
      smb_encryption: 'required',
      durable_handles: true,
      multichannel: false,
      oplocks: false,
      leases: false,
      mac_compat: false,
      veto_files: '/*.bak/~$*/',
      delete_veto_files: true,
      follow_symlinks: false,
    })
  })

  it('does not save a pattern Samba would misread, and shows a refusal', async () => {
    on('PUT', '/api/v1/file-services', () => problem(409, 'refused', 'testparm said no'))
    await openOptions()
    await click('More')
    await type('input[aria-label="Pattern"]', 'no slashes')
    expect(page()).toContain('Names between slashes')
    await click('Save')
    expect(callsTo('PUT', '/api/v1/file-services')).toHaveLength(0)
    await type('input[aria-label="Pattern"]', '/*.tmp/')
    await tick('Keep files out (veto files)', false)
    await click('Save')
    expect(page()).toContain('testparm said no')
  })
})
