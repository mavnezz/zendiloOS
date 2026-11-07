import {
  answer,
  callsTo,
  cleanup,
  click,
  mountApp,
  on,
  page,
  press,
  problem,
  settle,
  type,
} from '@/shared/testing/server'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { FileVolumeDto } from '@/shared/api/types'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { entry, fileShare, job, share } from '@/shared/testing/fixtures'

import FileExplorer from './FileExplorer.vue'

/** XMLHttpRequest as uploads use it, answered by the test. */
class FakeUpload {
  static sent: FakeUpload[] = []
  upload: { onprogress: ((event: ProgressEvent) => void) | null } = { onprogress: null }
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null
  status = 0
  responseText = ''
  url = ''
  open(_method: string, url: string): void {
    this.url = url
  }
  setRequestHeader(): void {}
  send(): void {
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

function folder(path: string, entries = [entry('b.txt')], extra = {}) {
  return { path, entries, truncated: false, writable: true, ...extra }
}

function volumes(admin = false): FileVolumeDto[] {
  const folders = [
    {
      id: 1,
      name: 'photos',
      description: null,
      shared: true,
      access: 'read_write',
      recycle_bin: true,
    },
    { id: 3, name: 'media', description: null, shared: false, access: 'read', recycle_bin: false },
  ] as FileVolumeDto['folders']
  if (admin) {
    folders.push({
      id: 4,
      name: 'secret',
      description: null,
      shared: true,
      access: null,
      recycle_bin: true,
    })
  }
  return [{ volume: 1, pool: 'tank', mounted: true, folders }]
}

const SNAPSHOT = 'zdl-2026-11-01_10.00.00'

function serve(admin = false): void {
  answer('GET', '/api/v1/files/shares', [
    fileShare(0, 'home', { home: true }),
    fileShare(1, 'photos'),
    fileShare(3, 'media', { access: 'read', recycle_bin: false }),
  ])
  answer('GET', '/api/v1/files/volumes', volumes(admin))
  answer('GET', '/api/v1/files/favourites', [])
  on('PUT', '/api/v1/files/favourites', ({ body }) =>
    (body as { favourites: { share_id: number; path: string }[] }).favourites.map((f) => ({
      ...f,
      share_name: 'photos',
    })),
  )
  answer('GET', '/api/v1/files/shares/1/snapshots', [
    { label: SNAPSHOT, taken_at: 1_700_000_000, note: 'before' },
  ])
  answer('GET', '/api/v1/files/shares/3/snapshots', [])
  on('GET', '/api/v1/files/shares/0/folder', ({ query }) =>
    query.get('path') === ''
      ? folder('', [entry('Docs', 'directory'), entry('notes.txt')])
      : problem(404, 'not_found'),
  )
  on('GET', '/api/v1/files/shares/1/folder', ({ query }) => {
    const path = query.get('path') ?? ''
    if (path === '') {
      return folder('', [
        entry('b.txt', 'file', { size: 10 }),
        entry('Album', 'directory'),
        entry('a.jpg', 'file', { size: 5000 }),
        entry('.hidden'),
        entry('link', 'symlink'),
        entry('data.bin', 'file', { size: 1 }),
        entry('bad-�.txt', 'file', { unreadable: true }),
      ])
    }
    if (path === 'Album') return folder('Album', [entry('c.png')])
    if (path === '.recycle/anna') return folder(path, [entry('old.txt')])
    return folder(path, [])
  })
  on('GET', '/api/v1/files/shares/3/folder', () =>
    folder('', [entry('film.mkv')], { writable: false }),
  )
  on('POST', '/api/v1/files/shares/1/folders', () => ({}))
  on('POST', '/api/v1/files/shares/1/rename', () => ({}))
  on('POST', '/api/v1/files/shares/1/delete', () => job(5, 'files.delete'))
  on('POST', '/api/v1/files/shares/1/transfer', () => job(6, 'files.copy'))
  on('POST', '/api/v1/files/shares/1/restore', ({ body }) => ({
    paths: (body as { paths: string[] }).paths,
  }))
  on('POST', `/api/v1/files/shares/1/snapshots/${SNAPSHOT}/restore`, () => job(7, 'files.restore'))
  if (admin) {
    answer('GET', '/api/v1/shares', [
      share(1, 'photos'),
      share(3, 'media', { shared: false }),
      share(4, 'secret'),
    ])
    answer('GET', '/api/v1/accounts', [])
    answer('GET', '/api/v1/groups', [])
    on('POST', '/api/v1/shares', ({ body }) => share(9, (body as { name: string }).name))
    on('PATCH', '/api/v1/shares/1', () => ({ share: share(1, 'photos'), job: null }))
    on('PATCH', '/api/v1/shares/3', () => ({ share: share(3, 'media'), job: null }))
    on('POST', '/api/v1/shares/3/delete', () => job(8, 'sharing.delete'))
  }
}

async function show(admin = false) {
  const wrapper = mountApp(FileExplorer, {
    props: { userName: 'anna', accountId: 7, canManage: admin },
  })
  await settle()
  return wrapper
}

function rows(): HTMLTableRowElement[] {
  return [...document.body.querySelectorAll<HTMLTableRowElement>('tbody tr')]
}
function names(): string[] {
  return rows().map((r) => r.querySelector('td + td .truncate')!.textContent.trim())
}
function row(name: string): HTMLTableRowElement {
  const found = rows().find(
    (r) => r.querySelector('td + td .truncate')!.textContent.trim() === name,
  )
  if (!found) throw new Error(`no row ${name} among ${names().join(', ')}`)
  return found
}
async function fire(element: Element, kind: string, init: MouseEventInit = {}): Promise<void> {
  element.dispatchEvent(new MouseEvent(kind, { bubbles: true, cancelable: true, ...init }))
  await settle()
}
async function key(name: string, init: KeyboardEventInit = {}): Promise<void> {
  const entries = document.body.querySelector('[aria-label="Entries"]')!
  entries.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, ...init }))
  await settle()
}
function tool(label: string): HTMLButtonElement {
  const found = document.body.querySelector<HTMLButtonElement>(
    `[role="toolbar"] [aria-label="${label}"]`,
  )
  if (!found) throw new Error(`no tool ${label}`)
  return found
}
async function use(label: string): Promise<void> {
  tool(label).click()
  await settle()
}
/** The button with this text in the open dialog. */
async function confirm(text: string): Promise<void> {
  const dialog = document.body.querySelector('[role="alertdialog"], [role="dialog"]')!
  const found = [...dialog.querySelectorAll('button')].find((b) => b.textContent?.trim() === text)
  if (!found) throw new Error(`no ${text} in ${dialog.textContent}`)
  found.click()
  await settle()
}
async function menu(element: Element, item: string): Promise<void> {
  await fire(element, 'contextmenu', { clientX: 20, clientY: 20 })
  const items = [...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]')]
  const found = items.find((i) => i.textContent?.trim().startsWith(item))
  if (!found) throw new Error(`no ${item} among ${items.map((i) => i.textContent).join(', ')}`)
  found.click()
  await settle()
}
function treeButton(name: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll<HTMLButtonElement>('aside button')].find(
    (b) => b.textContent?.trim() === name,
  )
  if (!found) throw new Error(`no ${name} in the tree`)
  return found
}
async function goTo(name: string): Promise<void> {
  treeButton(name).click()
  await settle()
}
function dropEvent(data: { files?: File[]; entries?: unknown }, init: MouseEventInit = {}): Event {
  const store: Record<string, string> = {}
  if (data.entries) store['application/x-zendiloos-entries'] = JSON.stringify(data.entries)
  const event = new MouseEvent('drop', { bubbles: true, cancelable: true, ...init })
  Object.defineProperty(event, 'dataTransfer', {
    value: {
      files: data.files ?? [],
      getData: (t: string) => store[t] ?? '',
      setData: () => undefined,
    },
  })
  return event
}
function notified(text: string): boolean {
  return useNotifications().items.some((n) => n.title.includes(text))
}

let saved: string[] = []
beforeEach(() => {
  cleanup()
  saved = []
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
    this: HTMLAnchorElement,
  ) {
    saved.push(this.getAttribute('href') ?? '')
  })
  FakeUpload.sent = []
  vi.stubGlobal('XMLHttpRequest', FakeUpload)
})
afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('the places', () => {
  it('opens the personal folder and shows favourites, volumes and the recycle bin', async () => {
    serve()
    answer('GET', '/api/v1/files/favourites', [
      { share_id: 1, path: 'Album', share_name: 'photos' },
    ])
    await show()
    expect(names()).toEqual(['Docs', 'notes.txt'])
    for (const place of [
      'Personal folder',
      'Album',
      'Volume 1 (tank)',
      'photos',
      'media',
      'Recycle bin',
    ]) {
      expect(treeButton(place)).toBeTruthy()
    }
    await goTo('Album')
    expect(names()).toEqual(['c.png'])
    expect(document.body.querySelector('ol[aria-label="Location"]')!.textContent).toContain('Album')
  })

  it('shows the folders below a folder of the tree once opened', async () => {
    serve()
    await show()
    await press('Show the folders in photos')
    expect(treeButton('Album')).toBeTruthy()
    await press('Hide the folders in photos')
    expect(() => treeButton('Album')).toThrow()
  })

  it('says when nothing is open to the person, and when it cannot be read', async () => {
    answer('GET', '/api/v1/files/shares', [])
    answer('GET', '/api/v1/files/volumes', [])
    answer('GET', '/api/v1/files/favourites', [])
    await show()
    expect(page()).toContain('No folder is open to you yet.')

    cleanup()
    serve()
    on('GET', '/api/v1/files/shares/0/folder', () => problem(403, 'permission_denied'))
    await show()
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })
})

describe('browsing', () => {
  it('goes into folders and back, up, forward; hides hidden files; sorts and searches', async () => {
    serve()
    await show()
    await goTo('photos')
    expect(names()).toEqual(['Album', 'a.jpg', 'b.txt', `bad-�.txt`, 'data.bin', 'link'])
    await press('Show hidden files')
    expect(names()).toContain('.hidden')
    await press('Hide hidden files')

    await fire(row('Album'), 'dblclick')
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').at(-1)!.query.get('path')).toBe('Album')
    await press('Up one folder')
    expect(names()).toContain('b.txt')
    await press('Back')
    expect(names()).toEqual(['c.png'])
    await press('Forward')
    expect(names()).toContain('b.txt')
    await press('Up one folder')
    expect(page()).toContain('Folder on the volume')
    await key('Backspace')
    expect(names()).toEqual(['media', 'photos'])
    await key('ArrowLeft', { altKey: true })
    expect(names()).toContain('b.txt')

    await click('Size')
    expect(names()).toEqual(['Album', 'link', 'data.bin', 'b.txt', `bad-\uFFFD.txt`, 'a.jpg'])
    await click('Size')
    expect(names()[1]).toBe('a.jpg')
    await click('Type')
    expect(page()).toContain('TXT file')
    await type('input[type="search"]', 'a.j')
    expect(names()).toEqual(['a.jpg'])
  })

  it('shows names that are not text, never to act on', async () => {
    serve()
    await show()
    await goTo('photos')
    const bad = row(`bad-�.txt`)
    expect(bad.querySelector('input')).toBeNull()
    await fire(bad, 'click')
    expect(tool('Rename').disabled).toBe(true)
  })

  it('opens previews and saves files and folders', async () => {
    serve()
    await show()
    await goTo('photos')
    await fire(row('a.jpg'), 'dblclick')
    expect(document.body.querySelector('[role="dialog"]')!.textContent).toContain('a.jpg')
    await press('Close')
    await fire(row('data.bin'), 'dblclick')
    expect(saved).toEqual(['/api/v1/files/shares/1/download?path=data.bin'])
    await fire(row('Album'), 'click')
    await use('Download')
    expect(saved.at(-1)).toBe('/api/v1/files/shares/1/archive?path=Album')
  })
})

describe('choosing and acting', () => {
  it('chooses with clicks, Ctrl, Shift and Ctrl+A; the toolbar follows', async () => {
    serve()
    await show()
    await goTo('photos')
    expect(tool('Rename').disabled).toBe(true)
    await fire(row('b.txt'), 'click')
    expect(tool('Rename').disabled).toBe(false)
    await fire(row('a.jpg'), 'click', { ctrlKey: true })
    expect(tool('Rename').disabled).toBe(true)
    expect(tool('Delete').disabled).toBe(false)
    await fire(row('link'), 'click', { shiftKey: true })
    expect(rows().filter((r) => r.getAttribute('aria-selected') === 'true')).toHaveLength(4)
    await key('a', { ctrlKey: true })
    // Every entry but the one whose name is not text (and the hidden one).
    expect(rows().filter((r) => r.getAttribute('aria-selected') === 'true')).toHaveLength(5)
    await key('a', { ctrlKey: true })
    expect(rows().filter((r) => r.getAttribute('aria-selected') === 'true')).toHaveLength(0)
    await fire(row('Album'), 'click')
    await key('ArrowDown')
    expect(row('a.jpg').getAttribute('aria-selected')).toBe('true')
  })

  it('makes folders, renames and deletes, from the toolbar and by keys', async () => {
    serve()
    await show()
    await goTo('photos')
    await use('New folder')
    await type('#file-name', 'New')
    await confirm('Create')
    expect(callsTo('POST', '/api/v1/files/shares/1/folders')[0]!.body).toEqual({ path: 'New' })

    await fire(row('b.txt'), 'click')
    await key('F2')
    expect(document.body.querySelector<HTMLInputElement>('#file-name')!.value).toBe('b.txt')
    await type('#file-name', 'c.txt')
    await confirm('Rename')
    expect(callsTo('POST', '/api/v1/files/shares/1/rename')[0]!.body).toEqual({
      path: 'b.txt',
      name: 'c.txt',
    })

    await fire(row('b.txt'), 'click')
    await key('Delete')
    expect(page()).toContain('Move to the recycle bin?')
    await confirm('Delete')
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({ paths: ['b.txt'] })
    expect(notified('Deleting 1 item')).toBe(true)
  })

  it('copies through the context menu and pastes by keys into another folder', async () => {
    serve()
    await show()
    await goTo('photos')
    await menu(row('a.jpg'), 'Copy')
    expect(page()).toContain('1 item copied')
    await fire(row('Album'), 'dblclick')
    await key('v', { ctrlKey: true })
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[0]!.body).toEqual({
      mode: 'copy',
      paths: ['a.jpg'],
      target_share: 1,
      target_path: 'Album',
    })
    await press('Forget the copied items')
    expect(page()).not.toContain('item copied')

    await fire(row('c.png'), 'click')
    await key('x', { ctrlKey: true })
    await goTo('photos')
    await use('Paste')
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[1]!.body).toMatchObject({
      mode: 'move',
      paths: ['Album/c.png'],
      target_path: '',
    })
  })

  it('acts on a folder of the tree from its context menu', async () => {
    serve()
    await show()
    await press('Show the folders in photos')
    await menu(treeButton('Album'), 'New folder')
    await type('#file-name', 'Inner')
    await confirm('Create')
    expect(callsTo('POST', '/api/v1/files/shares/1/folders')[0]!.body).toEqual({
      path: 'Album/Inner',
    })
    await menu(treeButton('Album'), 'Add to favourites')
    expect(callsTo('PUT', '/api/v1/files/favourites')[0]!.body).toEqual({
      favourites: [{ share_id: 1, path: 'Album' }],
    })
  })

  it('moves what is dropped on a folder, copies with Ctrl, and uploads files dropped', async () => {
    serve()
    await show()
    await goTo('photos')
    row('Album').dispatchEvent(dropEvent({ entries: { share: 1, paths: ['b.txt'] } }))
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[0]!.body).toMatchObject({
      mode: 'move',
      paths: ['b.txt'],
      target_path: 'Album',
    })
    row('Album').dispatchEvent(
      dropEvent({ entries: { share: 1, paths: ['b.txt'] } }, { ctrlKey: true }),
    )
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[1]!.body).toMatchObject({
      mode: 'copy',
    })
    row('Album').dispatchEvent(dropEvent({ entries: { share: 1, paths: ['Album'] } }))
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')).toHaveLength(2)

    const entries = document.body.querySelector('[aria-label="Entries"]')!
    entries.dispatchEvent(dropEvent({ files: [new File(['x'], 'new.txt')] }))
    await settle()
    expect(FakeUpload.sent[0]!.url).toContain('/api/v1/files/shares/1/upload?path=&name=new.txt')
    FakeUpload.sent[0]!.answer(204)
    await settle()
    expect(page()).not.toContain('Uploading new.txt')
  })

  it('uploads what is picked, asking before replacing', async () => {
    serve()
    await show()
    await goTo('photos')
    await use('Upload')
    const input = document.body.querySelector<HTMLInputElement>('input[type="file"]')!
    Object.defineProperty(input, 'files', {
      value: [new File(['x'], 'b.txt'), new File(['y'], 'z.txt')],
      configurable: true,
    })
    input.dispatchEvent(new Event('change'))
    await settle()
    expect(page()).toContain('This folder already contains: b.txt.')
    await confirm('Replace')
    expect(FakeUpload.sent.map((u) => u.url)).toEqual([
      '/api/v1/files/shares/1/upload?path=&name=b.txt&replace=true',
      '/api/v1/files/shares/1/upload?path=&name=z.txt&replace=false',
    ])
    FakeUpload.sent[0]!.answer(500, '{"code":"internal"}')
    await settle()
    await press('Dismiss')
    await press('Cancel the upload')
    expect(document.body.querySelector('[aria-label="Dismiss"]')).toBeNull()
  })

  it('keeps what is read only from being changed', async () => {
    serve()
    await show()
    await goTo('media')
    await fire(row('film.mkv'), 'click')
    expect(tool('Delete').disabled).toBe(true)
    expect(tool('Upload').disabled).toBe(true)
    expect(tool('Copy').disabled).toBe(false)
  })

  it('tells what an entry is', async () => {
    serve()
    await show()
    await goTo('photos')
    await fire(row('b.txt'), 'click')
    await use('Properties')
    const dialog = document.body.querySelector('[role="dialog"]')!.textContent
    expect(dialog).toContain('TXT file')
    expect(dialog).toContain('photos/b.txt')
  })
})

describe('recycle bins and previous versions', () => {
  it('gathers the person’s bins, restores from them and empties them', async () => {
    serve()
    on('GET', '/api/v1/files/shares/0/folder', ({ query }) =>
      query.get('path') === '.recycle/anna' ? problem(404, 'not_found') : folder('', []),
    )
    await show()
    await goTo('Recycle bin')
    expect(names()).toEqual(['old.txt'])
    expect(page()).toContain('Deleted from')
    await fire(row('old.txt'), 'click')
    await use('Restore')
    expect(callsTo('POST', '/api/v1/files/shares/1/restore')[0]!.body).toEqual({
      paths: ['.recycle/anna/old.txt'],
    })
    await use('Empty the recycle bin')
    await confirm('Empty the recycle bin')
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({
      paths: ['.recycle/anna/old.txt'],
    })
  })

  it('looks into a snapshot and copies back from it', async () => {
    serve()
    await show()
    await goTo('photos')
    await use('Previous versions')
    expect(page()).toContain('before')
    await confirm('Open')
    expect(page()).toContain('As it was on')
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').at(-1)!.query.get('snapshot')).toBe(
      SNAPSHOT,
    )
    await fire(row('b.txt'), 'click')
    await use('Copy back')
    expect(
      callsTo('POST', `/api/v1/files/shares/1/snapshots/${SNAPSHOT}/restore`)[0]!.body,
    ).toEqual({
      paths: ['b.txt'],
    })
    await click('Back to now')
    expect(page()).not.toContain('As it was on')
  })

  it('refreshes when a job of the files is over', async () => {
    serve()
    await show()
    const before = callsTo('GET', '/api/v1/files/shares/0/folder').length
    useJobsStore().update(job(1, 'files.copy', { state: 'succeeded' }))
    await settle()
    expect(callsTo('GET', '/api/v1/files/shares/0/folder').length).toBeGreaterThan(before)
  })
})

describe('folders on a volume', () => {
  it('lists them for administrators, those they cannot open marked', async () => {
    serve(true)
    await show(true)
    await goTo('Volume 1 (tank)')
    expect(names()).toEqual(['media', 'photos', 'secret'])
    expect(page()).toContain('Shared folder')
    expect(page()).toContain('Folder on the volume')
    await fire(row('secret'), 'click')
    expect(tool('Open').disabled).toBe(true)
    expect(treeButton('secret').disabled).toBe(true)
  })

  it('makes one, off the network and for its maker only', async () => {
    serve(true)
    await show(true)
    await goTo('Volume 1 (tank)')
    await use('New folder')
    expect(page()).toContain('A folder directly on a volume')
    await type('#file-name', 'docker')
    await confirm('Create')
    expect(callsTo('POST', '/api/v1/shares')[0]!.body).toEqual({
      name: 'docker',
      description: null,
      pool: 'tank',
      shared: false,
      recycle_bin: true,
      access: [{ kind: 'account', id: 7, access: 'read_write' }],
    })
  })

  it('shares one on the network and stops sharing another', async () => {
    serve(true)
    await show(true)
    await goTo('Volume 1 (tank)')
    await fire(row('media'), 'click')
    await use('Share on the network')
    expect(callsTo('PATCH', '/api/v1/shares/3')[0]!.body).toEqual({ shared: true })
    expect(notified('media is shared on the network now')).toBe(true)
    await menu(row('photos'), 'Stop sharing')
    expect(callsTo('PATCH', '/api/v1/shares/1')[0]!.body).toEqual({ shared: false })
  })

  it('sets its properties in tabs, and deletes it by its name', async () => {
    serve(true)
    await show(true)
    await goTo('Volume 1 (tank)')
    await fire(row('photos'), 'click')
    await use('Properties')
    expect(page()).toContain('Properties of photos')
    await click('Permissions')
    expect(page()).toContain('In Files, for apps and over the network alike.')
    await click('Sharing')
    const shared = [
      ...document.body.querySelectorAll<HTMLLabelElement>('[role="dialog"] label'),
    ].find((l) => l.textContent?.includes('Share on the network'))!
    shared.querySelector('input')!.click()
    await settle()
    await confirm('Save')
    expect(callsTo('PATCH', '/api/v1/shares/1')[0]!.body).toMatchObject({
      shared: false,
      recycle_bin: true,
    })

    await fire(row('media'), 'click')
    await use('Delete')
    expect(page()).toContain('Delete media?')
    await type('#folder-confirm', 'media')
    await confirm('Delete the folder and all files')
    expect(callsTo('POST', '/api/v1/shares/3/delete')[0]!.body).toEqual({ confirm: 'media' })
  })

  it('offers managing the snapshots of one to administrators', async () => {
    serve(true)
    answer('GET', '/api/v1/shares/1/snapshots', { snapshots: [], schedules: [] })
    await show(true)
    await goTo('photos')
    await use('Previous versions')
    await confirm('Manage snapshots')
    expect(page()).toContain('Snapshots of photos')
  })
})
