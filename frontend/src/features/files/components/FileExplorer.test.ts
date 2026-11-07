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

import { FOLDER_POLL_MS, VOLUMES_POLL_MS } from '../api'
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

describe('searching', () => {
  /** Types `text` and waits until the NAS is asked. */
  async function search(text: string): Promise<void> {
    await type('input[type="search"]', text)
    await new Promise((resolve) => setTimeout(resolve, 350))
    await settle()
  }
  function found(path: string) {
    const name = path.split('/').pop()!
    return { path, ...entry(name, name.includes('.') ? 'file' : 'directory') }
  }

  it('looks below the folder on the NAS and opens what it found where it is', async () => {
    serve()
    on('GET', '/api/v1/files/shares/1/search', ({ query }) => ({
      path: query.get('path'),
      entries:
        query.get('text') === 'many'
          ? [found('Album/c.png')]
          : [found('Album/beach.jpg'), found('Album/2024/Beach'), found('a.jpg')],
      stopped: query.get('text') === 'many' ? 'results' : null,
    }))
    await show()
    await goTo('photos')

    await search('beach')
    const asked = callsTo('GET', '/api/v1/files/shares/1/search').at(-1)!.query
    expect([asked.get('path'), asked.get('text'), asked.get('hidden')]).toEqual(['', 'beach', null])
    expect(names()).toEqual(['Beach', 'a.jpg', 'beach.jpg'])
    expect(page()).toContain('In folder')
    expect(page()).toContain('3 entries named like “beach” below this folder')
    expect(row('beach.jpg').textContent).toContain('Album')

    await fire(row('beach.jpg'), 'click')
    await menu(row('beach.jpg'), 'Open its folder')
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').at(-1)!.query.get('path')).toBe('Album')
    expect((document.body.querySelector('input[type="search"]') as HTMLInputElement).value).toBe('')

    await press('Show hidden files')
    await search('many')
    const again = callsTo('GET', '/api/v1/files/shares/1/search').at(-1)!.query
    expect([again.get('path'), again.get('hidden')]).toEqual(['Album', 'true'])
    expect(page()).toContain('The first 1 found')
    await click('Clear the search')
    expect(names()).toContain('c.png')
  })

  it('says when the search fails, and filters what is shown meanwhile', async () => {
    serve()
    on('GET', '/api/v1/files/shares/1/search', () => problem(422, 'invalid_operation'))
    await show()
    await goTo('photos')
    await search('a.j')
    expect(page()).toContain('The search failed')
    expect(names()).toEqual(['a.jpg'])
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

  it('opens folders in a bin, restores and deletes at any depth, and goes back up', async () => {
    serve()
    on('GET', '/api/v1/files/shares/0/folder', ({ query }) =>
      query.get('path') === '.recycle/anna' ? problem(404, 'not_found') : folder('', []),
    )
    on('GET', '/api/v1/files/shares/1/folder', ({ query }) => {
      const path = query.get('path')
      if (path === '.recycle/anna') return folder(path, [entry('Album', 'directory')])
      if (path === '.recycle/anna/Album') return folder(path, [entry('2024', 'directory')])
      if (path === '.recycle/anna/Album/2024') return folder(path, [entry('deep.jpg')])
      return folder(path ?? '', [])
    })
    await show()
    await goTo('Recycle bin')
    await fire(row('Album'), 'dblclick')
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').at(-1)!.query.get('path')).toBe(
      '.recycle/anna/Album',
    )
    await fire(row('2024'), 'dblclick')
    expect(names()).toEqual(['deep.jpg'])
    expect(row('deep.jpg').textContent).toContain('photos/Album/2024')
    const crumbs = document.body.querySelector('[aria-label="Location"]')!.textContent
    expect(crumbs).toContain('Recycle bin')
    expect(crumbs).toContain('photos')
    expect(crumbs).toContain('2024')

    await fire(row('deep.jpg'), 'click')
    expect(() => tool('Empty the recycle bin')).not.toThrow()
    expect(tool('Empty the recycle bin').disabled).toBe(true)
    await use('Restore')
    expect(callsTo('POST', '/api/v1/files/shares/1/restore')[0]!.body).toEqual({
      paths: ['.recycle/anna/Album/2024/deep.jpg'],
    })
    await fire(row('deep.jpg'), 'click')
    await use('Delete')
    await confirm('Delete')
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({
      paths: ['.recycle/anna/Album/2024/deep.jpg'],
    })

    await press('Up one folder')
    expect(names()).toEqual(['2024'])
    await press('Up one folder')
    await press('Up one folder')
    expect(names()).toEqual(['Album'])
    expect(document.body.querySelector('[aria-label="Location"]')!.textContent?.trim()).toBe(
      'Recycle bin',
    )
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

  it('reads the folder shown and the volumes again by themselves', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      serve()
      await show()
      const folders = callsTo('GET', '/api/v1/files/shares/0/folder').length
      const volumes = callsTo('GET', '/api/v1/files/volumes').length
      await vi.advanceTimersByTimeAsync(FOLDER_POLL_MS)
      await settle()
      expect(callsTo('GET', '/api/v1/files/shares/0/folder').length).toBeGreaterThan(folders)
      await vi.advanceTimersByTimeAsync(VOLUMES_POLL_MS)
      await settle()
      expect(callsTo('GET', '/api/v1/files/volumes').length).toBeGreaterThan(volumes)
    } finally {
      vi.useRealTimers()
    }
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

describe('removable drives', () => {
  const STICK = '/api/v1/files/removable/1A2B-3C4D'

  function stick(state: 'mounted' | 'ejected' | 'failed' = 'mounted') {
    return {
      id: '1A2B-3C4D',
      label: 'STICK',
      filesystem: 'fat',
      device: 'sdf1',
      disk: 'sdf',
      model: 'TEST Stick',
      size_bytes: 32_000_000_000,
      state,
      read_only: false,
      path: state === 'mounted' ? '/run/zendiloos-removable/1A2B-3C4D' : null,
      used_bytes: state === 'mounted' ? 1_000_000 : null,
      available_bytes: state === 'mounted' ? 31_000_000_000 : null,
      problem: state === 'failed' ? 'the file system is damaged' : null,
    }
  }

  function serveDrives(state: 'mounted' | 'ejected' | 'failed' = 'mounted'): void {
    serve(true)
    answer('GET', '/api/v1/storage/removable', [stick(state)])
    on('GET', `${STICK}/folder`, ({ query }) =>
      (query.get('path') ?? '') === ''
        ? folder('', [entry('song.mp3', 'file', { size: 4 }), entry('Photos', 'directory')])
        : folder(query.get('path') ?? '', []),
    )
    on('POST', `${STICK}/transfer`, () => job(11, 'files.copy'))
    on('POST', `${STICK}/delete`, () => job(12, 'files.delete'))
    on('POST', '/api/v1/storage/removable/1A2B-3C4D/eject', () => null)
    on('POST', '/api/v1/storage/removable/1A2B-3C4D/mount', () => stick())
  }

  async function showWithDrives() {
    const wrapper = mountApp(FileExplorer, {
      props: { userName: 'anna', accountId: 7, canManage: true, canManageStorage: true },
    })
    await settle()
    return wrapper
  }

  function node(name: string): HTMLElement {
    const found = [...document.body.querySelectorAll<HTMLButtonElement>('aside button')].find((b) =>
      b.textContent?.trim().startsWith(name),
    )
    if (!found) throw new Error(`no ${name} in the tree`)
    return found.parentElement!
  }

  it('are shown only to those who manage storage', async () => {
    serve(true)
    await show(true)
    expect(page()).not.toContain('Removable drives')
    expect(callsTo('GET', '/api/v1/storage/removable')).toHaveLength(0)
  })

  it('are opened, and their files copied into a folder on a volume and back', async () => {
    serveDrives()
    await showWithDrives()
    expect(page()).toContain('Removable drives')
    await goTo('STICK')
    expect(names()).toEqual(['Photos', 'song.mp3'])
    expect(page()).not.toContain('Add to favourites')

    await fire(row('song.mp3'), 'click')
    await key('c', { ctrlKey: true })
    await goTo('photos')
    await key('v', { ctrlKey: true })
    expect(callsTo('POST', `${STICK}/transfer`)[0]!.body).toEqual({
      mode: 'copy',
      paths: ['song.mp3'],
      target_share: 1,
      target_path: '',
    })

    // Dragged onto the drive in the tree: it lights up, then gets it.
    const target = node('STICK')
    await fire(target, 'dragover')
    expect(target.dataset.over).toBe('true')
    target.dispatchEvent(dropEvent({ entries: { share: 1, paths: ['b.txt'] } }))
    await settle()
    expect(target.dataset.over).toBeUndefined()
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[0]!.body).toEqual({
      mode: 'move',
      paths: ['b.txt'],
      target_drive: '1A2B-3C4D',
      target_path: '',
    })
  })

  it('delete for good and upload onto the drive', async () => {
    serveDrives()
    await showWithDrives()
    await goTo('STICK')
    await fire(row('song.mp3'), 'click')
    await use('Delete')
    expect(page()).toContain('for good')
    await confirm('Delete')
    expect(callsTo('POST', `${STICK}/delete`)[0]!.body).toEqual({ paths: ['song.mp3'] })

    const entries = document.body.querySelector('[aria-label="Entries"]')!
    entries.dispatchEvent(dropEvent({ files: [new File(['x'], 'new.txt')] }))
    await settle()
    expect(FakeUpload.sent[0]!.url).toContain(`${STICK}/upload?path=&name=new.txt`)
  })

  it('are ejected from the tree, saying when files are still open', async () => {
    serveDrives()
    await showWithDrives()
    on('POST', '/api/v1/storage/removable/1A2B-3C4D/eject', () => problem(409, 'drive_in_use'))
    await menu(node('STICK'), 'Eject')
    expect(notified('still open')).toBe(true)
    on('POST', '/api/v1/storage/removable/1A2B-3C4D/eject', () => null)
    await goTo('STICK')
    expect(names()).toContain('song.mp3')
    // Powered off, it is gone from the list once asked again.
    answer('GET', '/api/v1/storage/removable', [])
    await use('Eject')
    expect(callsTo('POST', '/api/v1/storage/removable/1A2B-3C4D/eject').at(-1)!.body).toEqual({
      power_off: true,
    })
    expect(notified('can be pulled out now')).toBe(true)
    // The explorer leaves it, for the personal folder.
    expect(names()).not.toContain('song.mp3')
    expect(names()).toContain('notes.txt')
  })

  it('are mounted again once ejected, and say why one cannot be', async () => {
    serveDrives('ejected')
    await showWithDrives()
    expect(node('STICK').textContent).toContain('Ejected')
    await menu(node('STICK'), 'Mount')
    expect(callsTo('POST', '/api/v1/storage/removable/1A2B-3C4D/mount')).toHaveLength(1)
    expect(notified('is mounted')).toBe(true)
  })

  it('show why one could not be mounted', async () => {
    serveDrives('failed')
    await showWithDrives()
    expect(node('STICK').textContent).toContain('Cannot be mounted')
    expect(node('STICK').querySelector('[title]')!.getAttribute('title')).toBe(
      'the file system is damaged',
    )
  })
})
