import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { VmImage, VmPlugin } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes } from '@/shared/lib/format'
import { button, click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import ImagesSection from './ImagesSection.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const PLUGIN: VmPlugin = {
  hardware: { kvm: true, cpu_virtualization: 'vmx', iommu: false },
  installed: true,
  storage: { pool: 'tank', volume: 1 },
  pools: [{ pool: 'tank', volume: 1 }],
  cores: 4,
  memory_bytes: 8 * 1024 ** 3,
  bridges: [],
  image_upload_max_bytes: 1000,
}

const IMAGES: VmImage[] = [
  {
    id: '00000000000a',
    name: 'debian-13.iso',
    bytes: 700 * 1024 ** 2,
    origin: { kind: 'share', share: 'media', path: 'isos/debian-13.iso' },
    added_at: 1_790_000_000,
    ready: true,
    used_by: ['web', 'mail'],
  },
  {
    id: '00000000000b',
    name: 'alpine',
    bytes: 60 * 1024 ** 2,
    origin: { kind: 'upload', file: 'alpine-virt.iso' },
    added_at: 1_790_000_100,
    ready: true,
    used_by: [],
  },
  {
    id: '00000000000c',
    name: 'win11.iso',
    bytes: null,
    origin: { kind: 'share', share: 'media', path: 'isos/win11.iso' },
    added_at: 1_790_000_200,
    ready: false,
    used_by: [],
  },
]

const JOB = {
  id: 9,
  kind: 'vms.image_add',
  target: 'debian-13.iso',
  state: 'running',
  step: null,
  progress: null,
  error_code: null,
  error_detail: null,
  created_at: 0,
  updated_at: 0,
  finished_at: null,
  output: [],
}

/** XMLHttpRequest as the upload uses it, answered by the test. */
class FakeUpload {
  static sent: FakeUpload[] = []
  upload: {
    onprogress:
      ((event: { lengthComputable: boolean; loaded: number; total: number }) => void) | null
  } = {
    onprogress: null,
  }
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null
  status = 0
  responseText = ''
  url = ''
  body: unknown = null
  open(_method: string, url: string): void {
    this.url = url
  }
  setRequestHeader(): void {}
  send(body: unknown): void {
    this.body = body
    FakeUpload.sent.push(this)
  }
  abort(): void {
    this.onabort?.()
  }
  answer(status: number, body = ''): void {
    this.status = status
    this.responseText = body
    this.onload?.()
  }
}

function row(id: string): string {
  return document.querySelector(`[data-image="${id}"]`)?.textContent ?? ''
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

beforeEach(() => {
  reset()
  FakeUpload.sent = []
  vi.stubGlobal('XMLHttpRequest', FakeUpload)
  on('GET', '/api/v1/vms/images', IMAGES)
  on('GET', '/api/v1/files/shares', [{ id: 3, name: 'media', access: 'read', home: false }])
  on('GET', '/api/v1/files/shares/3/folder', (call: { query: Record<string, string> }) => ({
    entries:
      call.query.path === 'isos'
        ? [{ name: 'debian-13.iso', kind: 'file' }]
        : [{ name: 'isos', kind: 'directory' }],
    writable: false,
  }))
})

afterEach(() => vi.unstubAllGlobals())

async function mountImages() {
  return mountApp(ImagesSection, { props: { plugin: PLUGIN } })
}

describe('the image library', () => {
  it('lists each image with its size, where it came from and who uses it', async () => {
    await mountImages()
    expect(row('00000000000a')).toContain('debian-13.iso')
    expect(row('00000000000a')).toContain(formatBytes(700 * 1024 ** 2))
    expect(row('00000000000a')).toContain('media/isos/debian-13.iso')
    expect(row('00000000000a')).toContain('web')
    expect(row('00000000000a')).toContain('mail')
    expect(row('00000000000b')).toContain(t('vms.images.uploaded', { file: 'alpine-virt.iso' }))
    expect(row('00000000000b')).toContain(t('vms.images.unused'))
    expect(row('00000000000c')).toContain(t('vms.images.adding'))
  })

  it('says when it is empty', async () => {
    on('GET', '/api/v1/vms/images', [])
    await mountImages()
    expect(page()).toContain(t('vms.images.empty'))
  })

  it('adds an ISO from a share, named after its file', async () => {
    on('POST', '/api/v1/vms/images', { image: IMAGES[0], job: JOB })
    await mountImages()
    await click(t('vms.images.fromNas'))
    expect(button(t('vms.images.addDialog.submit')).disabled).toBe(true)
    await click(t('vms.images.addDialog.pick'))
    const tree = () => [...document.querySelectorAll('[role="treeitem"]')]
    const expand = (name: string) =>
      tree()
        .find((item) => item.textContent?.trim() === name)!
        .querySelector('button')!
        .click()
    expand('media')
    await settle()
    expand('isos')
    await settle()
    tree()
      .find((item) => item.textContent?.trim() === 'debian-13.iso')!
      .querySelectorAll('button')[1]!
      .click()
    await settle()
    await click(t('folderPicker.confirm'))
    expect(document.querySelector('[data-testid="vm-image-picked"]')?.textContent).toBe(
      'media/isos/debian-13.iso',
    )
    expect(document.querySelector<HTMLInputElement>('#vm-image-name')!.value).toBe('debian-13.iso')
    await type('#vm-image-name', ' Debian 13 ')
    await submit('form')
    expect(called('POST', '/api/v1/vms/images')[0]?.body).toEqual({
      share_id: 3,
      path: 'isos/debian-13.iso',
      name: 'Debian 13',
    })
    expect(useJobsStore().list.map((job) => job.id)).toContain(9)
  })

  it('uploads an ISO from this computer, and refuses one too large', async () => {
    await mountImages()
    await click(t('vms.images.upload'))
    const input = document.body.querySelector<HTMLInputElement>('#vm-image-upload-file')!
    const choose = async (file: File) => {
      Object.defineProperty(input, 'files', { value: [file], configurable: true })
      input.dispatchEvent(new Event('change'))
      await settle()
    }
    await choose(new File(['x'.repeat(2000)], 'huge.iso'))
    expect(page()).toContain(t('vms.images.uploadDialog.tooLarge', { max: formatBytes(1000) }))
    expect(button(t('vms.images.uploadDialog.submit')).disabled).toBe(true)
    await choose(new File(['iso'], 'alpine-virt.iso'))
    expect(document.querySelector<HTMLInputElement>('#vm-image-upload-name')!.value).toBe(
      'alpine-virt.iso',
    )
    await click(t('vms.images.uploadDialog.submit'))
    expect(FakeUpload.sent.map((upload) => upload.url)).toEqual([
      '/api/v1/vms/images/upload?name=alpine-virt.iso',
    ])
    FakeUpload.sent[0]!.upload.onprogress?.({ lengthComputable: true, loaded: 1, total: 2 })
    await settle()
    expect(document.querySelector('[data-testid="vm-image-progress"]')?.textContent).toContain('50')
    FakeUpload.sent[0]!.answer(201, JSON.stringify(IMAGES[1]))
    await settle()
    expect(document.querySelector('#vm-image-upload-file')).toBeNull()
  })

  it('says the server refused an upload', async () => {
    await mountImages()
    await click(t('vms.images.upload'))
    const input = document.body.querySelector<HTMLInputElement>('#vm-image-upload-file')!
    Object.defineProperty(input, 'files', {
      value: [new File(['iso'], 'x.iso')],
      configurable: true,
    })
    input.dispatchEvent(new Event('change'))
    await settle()
    await click(t('vms.images.uploadDialog.submit'))
    FakeUpload.sent[0]!.answer(409, JSON.stringify({ code: 'name_taken' }))
    await settle()
    expect(page()).toContain(t('errors.name_taken'))
  })

  it('renames an image, and names the VMs that keep one from being deleted', async () => {
    on('PUT', '/api/v1/vms/images/00000000000a', { ...IMAGES[0], name: 'Debian 13' })
    on('DELETE', '/api/v1/vms/images/00000000000a', problem(409, 'image_in_use', 'web, mail'))
    on('DELETE', '/api/v1/vms/images/00000000000b', null)
    await mountImages()
    await menu(t('vms.images.more', { name: 'debian-13.iso' }), t('vms.images.rename'))
    await type('#vm-image-rename', 'Debian 13')
    await submit('form')
    expect(called('PUT', '/api/v1/vms/images/00000000000a')[0]?.body).toEqual({ name: 'Debian 13' })

    await menu(t('vms.images.more', { name: 'debian-13.iso' }), t('vms.images.delete'))
    expect(page()).toContain(t('vms.images.deleteText'))
    await click(t('vms.images.delete'))
    expect(document.querySelector('[data-testid="vm-images-failure"]')?.textContent).toContain(
      'web, mail',
    )
    await menu(t('vms.images.more', { name: 'alpine' }), t('vms.images.delete'))
    await click(t('vms.images.delete'))
    expect(called('DELETE', '/api/v1/vms/images/00000000000b')).toHaveLength(1)
  })
})
