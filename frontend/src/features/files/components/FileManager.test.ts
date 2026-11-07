import {
  answer,
  callsTo,
  choose,
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

import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { entry, fileShare, job } from '@/shared/testing/fixtures'

import FileManager from './FileManager.vue'

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
  headers: Record<string, string> = {}
  body: unknown = null

  open(_method: string, url: string): void {
    this.url = url
  }
  setRequestHeader(name: string, value: string): void {
    this.headers[name] = value
  }
  send(body: unknown): void {
    this.body = body
    FakeUpload.sent.push(this)
  }
  abort(): void {
    this.onabort?.()
  }
  progress(loaded: number, total: number): void {
    this.upload.onprogress?.({ lengthComputable: true, loaded, total } as ProgressEvent)
  }
  answer(status: number, body = ''): void {
    this.status = status
    this.responseText = body
    this.onload?.()
  }
}

const photos = fileShare(1, 'photos')
const home = fileShare(2, 'anna', { home: true })
const readOnly = fileShare(3, 'media', { access: 'read', recycle_bin: false })

function folder(path: string, entries = [entry('b.txt'), entry('Album', 'directory')], extra = {}) {
  return { path, entries, truncated: false, writable: true, ...extra }
}

function serve(): void {
  answer('GET', '/api/v1/files/shares', [photos, home, readOnly])
  answer('GET', '/api/v1/files/favourites', [])
  answer('GET', '/api/v1/files/shares/1/snapshots', [])
  answer('GET', '/api/v1/files/shares/2/snapshots', [])
  answer('GET', '/api/v1/files/shares/3/snapshots', [])
  on('GET', '/api/v1/files/shares/1/folder', ({ query }) => {
    const path = query.get('path') ?? ''
    if (path === '') {
      return folder('', [
        entry('b.txt'),
        entry('Album', 'directory'),
        entry('a.jpg'),
        entry('.hidden'),
        entry('link', 'symlink'),
        entry('data.bin'),
      ])
    }
    if (path === 'Album') return folder('Album', [entry('c.png')])
    if (path === '.recycle/anna') return problem(404, 'not_found')
    return folder(path, [])
  })
  on('GET', '/api/v1/files/shares/3/folder', () =>
    folder('', [entry('film.mkv')], { writable: false }),
  )
  on('GET', '/api/v1/files/shares/2/folder', () => folder('', [], { truncated: true }))
}

function row(name: string): HTMLInputElement {
  return document.body.querySelector<HTMLInputElement>(`input[aria-label="${name}"]`)!
}

async function select(...names: string[]): Promise<void> {
  for (const name of names) row(name).click()
  await settle()
}

function button(text: string): HTMLButtonElement | undefined {
  return [...document.body.querySelectorAll<HTMLButtonElement>('button')].find(
    (b) => b.textContent?.trim() === text,
  )
}

async function openEntry(name: string): Promise<void> {
  button(name)!.click()
  await settle()
}

async function show(userName = 'anna') {
  const wrapper = mountApp(FileManager, { props: { userName } })
  await settle()
  return wrapper
}

function notified(title: string): boolean {
  return useNotifications().items.some((n) => n.title.includes(title))
}

beforeEach(() => {
  cleanup()
  serve()
  FakeUpload.sent = []
  vi.stubGlobal('XMLHttpRequest', FakeUpload)
})

afterEach(() => vi.unstubAllGlobals())

describe('browsing', () => {
  it('lists folders first, hides hidden files until asked, and opens folders', async () => {
    await show()

    const names = [...document.body.querySelectorAll('tbody tr')].map((r) =>
      r.querySelector('td + td')!.textContent.trim(),
    )
    expect(names).toEqual(['Album', 'a.jpg', 'b.txt', 'data.bin', 'link'])
    await press('Show hidden files')
    expect(page()).toContain('.hidden')
    await press('Hide hidden files')

    await openEntry('Album')
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').at(-1)!.query.get('path')).toBe('Album')
    expect(document.body.querySelector('ol[aria-label="Location"]')!.textContent).toContain('Album')
    await press('Up one folder')
    expect(page()).toContain('b.txt')
    await openEntry('Album')
    await openEntry('photos')
    expect(page()).toContain('b.txt')
  })

  it('switches shares, from the list or the narrow select, and shows what is read only', async () => {
    await show()
    await openEntry('media')
    expect(page()).toContain('Read only')
    expect(button('Upload')!.disabled).toBe(true)
    await choose('select[aria-label="Shares"]', '2')
    expect(page()).toContain('Personal folder')
    expect(page()).toContain('Only the first 0 entries are shown.')
    expect(page()).toContain('This folder is empty.')
  })

  it('says when there are no shares, or they cannot be read', async () => {
    answer('GET', '/api/v1/files/shares', [])
    await show()
    expect(page()).toContain('No share is open to you yet.')

    cleanup()
    serve()
    on('GET', '/api/v1/files/shares', () => problem(500, 'internal'))
    await show()
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })

  it('shows a folder that cannot be read', async () => {
    on('GET', '/api/v1/files/shares/1/folder', () => problem(403, 'permission_denied'))
    await show()
    expect(document.body.querySelector('[role="alert"]')).not.toBeNull()
  })

  it('offers files and folders for download as selected', async () => {
    await show()
    await select('b.txt')
    expect(page()).toContain('1 selected')
    const download = () =>
      [...document.body.querySelectorAll<HTMLAnchorElement>('a')].find(
        (a) => a.textContent?.trim() === 'Download',
      )
    expect(download()!.getAttribute('href')).toBe('/api/v1/files/shares/1/download?path=b.txt')
    await select('b.txt', 'Album')
    expect(download()!.getAttribute('href')).toBe('/api/v1/files/shares/1/archive?path=Album')
    await press('Select all')
    expect(page()).toContain('5 selected')
    await press('Select all')
    expect(page()).not.toContain('selected')
  })

  it('shows a name that is not UTF-8 but offers nothing to do with it', async () => {
    on('GET', '/api/v1/files/shares/1/folder', () =>
      folder('', [entry('file-�.bin', 'file', { unreadable: true }), entry('file-�.bin')]),
    )
    on('POST', '/api/v1/files/shares/1/delete', () => job(6, 'files.delete'))
    await show()
    const boxes = document.body.querySelectorAll('input[aria-label="file-�.bin"]')
    expect(boxes).toHaveLength(1)
    expect(page()).toContain('file-�.bin')
    const marked = document.body.querySelector('[title^="This name is not valid text"]')!
    expect(marked.textContent).toContain('file-�.bin')
    expect(marked.querySelector('a, button')).toBeNull()
    const links = [...document.body.querySelectorAll<HTMLAnchorElement>('a[download]')]
    expect(links.filter((a) => a.textContent?.includes('file-'))).toHaveLength(1)

    await press('Select all')
    expect(page()).toContain('1 selected')
    const highlighted = document.body.querySelectorAll('tr.bg-accent\\/60')
    expect(highlighted).toHaveLength(1)
    expect(highlighted[0]!.querySelector('[title]')).toBeNull()
    await click('Delete')
    const confirm = [
      ...document.body.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button'),
    ].find((b) => b.textContent?.trim() === 'Delete')!
    confirm.click()
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({
      paths: ['file-�.bin'],
    })
  })

  it('reads the folder again when a files job ends', async () => {
    await show()
    const before = callsTo('GET', '/api/v1/files/shares/1/folder').length
    const jobs = useJobsStore()
    jobs.update(job(1, 'files.copy'))
    jobs.update(job(1, 'files.copy', { state: 'succeeded' }))
    await settle()
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').length).toBeGreaterThan(before)
  })
})

describe('changing files', () => {
  it('makes a folder, refusing names the server would refuse', async () => {
    on('POST', '/api/v1/files/shares/1/folders', ({ body }) =>
      (body as { path: string }).path === 'taken' ? problem(409, 'name_taken') : undefined,
    )
    await show()
    await click('New folder')
    await type('#file-name', 'a/b')
    expect(page()).toContain('A name cannot contain "/".')
    await type('#file-name', 'trailing.')
    expect(page()).toContain('A name cannot end with a dot')
    await type('#file-name', 'taken')
    await click('Create')
    expect(page()).toContain('This name is already taken')
    await type('#file-name', ' Trips ')
    await click('Create')
    expect(callsTo('POST', '/api/v1/files/shares/1/folders').at(-1)!.body).toEqual({
      path: 'Trips',
    })
    expect(document.body.querySelector('#file-name')).toBeNull()
  })

  it('renames an entry, and cancels', async () => {
    on('POST', '/api/v1/files/shares/1/rename', () => ({ path: 'c.txt' }))
    await show()
    await select('b.txt')
    await click('Rename')
    expect(document.body.querySelector<HTMLInputElement>('#file-name')!.value).toBe('b.txt')
    await click('Cancel')
    await click('Rename')
    await type('#file-name', 'c.txt')
    document.body.querySelector<HTMLInputElement>('#file-name')!.form!.requestSubmit()
    await settle()
    const renames = callsTo('POST', '/api/v1/files/shares/1/rename')
    expect(renames.at(-1)!.body).toEqual({ path: 'b.txt', name: 'c.txt' })
  })

  it('copies and moves through the clipboard, also into another share', async () => {
    on('POST', '/api/v1/files/shares/1/transfer', () => job(5, 'files.copy'))
    await show()
    await select('b.txt', 'a.jpg')
    await click('Copy')
    await openEntry('Album')
    await click('Paste 2 items here')
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[0]!.body).toEqual({
      mode: 'copy',
      paths: ['b.txt', 'a.jpg'],
      target_share: 1,
      target_path: 'Album',
    })
    expect(notified('Copying 2 items')).toBe(true)
    await press('Forget the copied items')
    expect(button('Paste 2 items here')).toBeUndefined()

    await press('Up one folder')
    await select('b.txt')
    await click('Cut')
    await openEntry('Personal folder')
    await click('Move 1 item here')
    expect(callsTo('POST', '/api/v1/files/shares/1/transfer')[1]!.body).toMatchObject({
      mode: 'move',
      target_share: 2,
      target_path: '',
    })
    expect(button('Move 1 item here')).toBeUndefined()
  })

  it('says when a transfer is refused', async () => {
    on('POST', '/api/v1/files/shares/1/transfer', () => problem(422, 'invalid_operation'))
    await show()
    await select('b.txt')
    await click('Copy')
    await click('Paste 1 item here')
    expect(useNotifications().items.some((n) => n.kind === 'error')).toBe(true)
  })

  it('moves into the recycle bin, or deletes for good where there is none', async () => {
    on('POST', '/api/v1/files/shares/1/delete', () => job(6, 'files.delete'))
    on('POST', '/api/v1/files/shares/3/delete', () => problem(403, 'permission_denied'))
    await show()
    await select('b.txt')
    await click('Delete')
    expect(page()).toContain('Move to the recycle bin?')
    const confirm = [
      ...document.body.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button'),
    ].find((b) => b.textContent?.trim() === 'Delete')!
    confirm.click()
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({ paths: ['b.txt'] })
    expect(notified('Deleting 1 item')).toBe(true)
  })
})

describe('the recycle bin', () => {
  it('is empty before the first deletion', async () => {
    await show()
    await press('Recycle bin')
    expect(page()).toContain('Your recycle bin in this share is empty.')
    expect(document.body.querySelector('ol[aria-label="Location"]')!.textContent).toContain(
      'Recycle bin',
    )
  })

  it('restores entries and empties itself for good', async () => {
    on('GET', '/api/v1/files/shares/1/folder', ({ query }) =>
      query.get('path') === '.recycle/anna'
        ? folder('.recycle/anna', [entry('old.txt'), entry('older.txt')])
        : folder(''),
    )
    on('POST', '/api/v1/files/shares/1/restore', () => ({ paths: ['old.txt'] }))
    on('POST', '/api/v1/files/shares/1/delete', () => job(7, 'files.delete'))
    await show()
    await press('Recycle bin')

    await select('old.txt')
    await click('Restore')
    expect(callsTo('POST', '/api/v1/files/shares/1/restore')[0]!.body).toEqual({
      paths: ['.recycle/anna/old.txt'],
    })
    expect(notified('Restored 1 item')).toBe(true)

    await select('older.txt')
    await click('Delete')
    expect(page()).toContain('Delete for good?')
    await click('Cancel')

    await click('Empty the recycle bin')
    expect(page()).toContain('Empty the recycle bin?')
    const confirm = [
      ...document.body.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button'),
    ].find((b) => b.textContent?.trim() === 'Empty the recycle bin')!
    confirm.click()
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({
      paths: ['.recycle/anna/old.txt', '.recycle/anna/older.txt'],
    })
  })

  it('leaves out a name that is not UTF-8 when it empties itself', async () => {
    on('GET', '/api/v1/files/shares/1/folder', ({ query }) =>
      query.get('path') === '.recycle/anna'
        ? folder('.recycle/anna', [
            entry('old�.txt', 'file', { unreadable: true }),
            entry('old.txt'),
          ])
        : folder(''),
    )
    on('POST', '/api/v1/files/shares/1/delete', () => job(7, 'files.delete'))
    await show()
    await press('Recycle bin')
    await click('Empty the recycle bin')
    const confirm = [
      ...document.body.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button'),
    ].find((b) => b.textContent?.trim() === 'Empty the recycle bin')!
    confirm.click()
    await settle()
    expect(callsTo('POST', '/api/v1/files/shares/1/delete')[0]!.body).toEqual({
      paths: ['.recycle/anna/old.txt'],
    })
  })

  it('says when restoring fails', async () => {
    on('GET', '/api/v1/files/shares/1/folder', () => folder('.recycle/anna', [entry('old.txt')]))
    on('POST', '/api/v1/files/shares/1/restore', () => problem(409, 'name_taken'))
    await show()
    await press('Recycle bin')
    await select('old.txt')
    await click('Restore')
    expect(useNotifications().items.some((n) => n.kind === 'error')).toBe(true)
  })
})

describe('favourites', () => {
  it('adds and removes the folder shown, and goes to one', async () => {
    let stored = [{ share_id: 1, path: 'Album', share_name: 'photos' }]
    on('GET', '/api/v1/files/favourites', () => stored)
    on('PUT', '/api/v1/files/favourites', ({ body }) => {
      stored = (body as { favourites: { share_id: number; path: string }[] }).favourites.map(
        (f) => ({
          ...f,
          share_name: 'photos',
        }),
      )
      return stored
    })
    await show()

    await press('Add this folder to the favourites')
    expect(callsTo('PUT', '/api/v1/files/favourites')[0]!.body).toEqual({
      favourites: [
        { share_id: 1, path: 'Album' },
        { share_id: 1, path: '' },
      ],
    })
    await press('Remove this folder from the favourites')
    expect(callsTo('PUT', '/api/v1/files/favourites')[1]!.body).toEqual({
      favourites: [{ share_id: 1, path: 'Album' }],
    })
    document.body.querySelector<HTMLButtonElement>('button[title="photos/Album"]')!.click()
    await settle()
    expect(page()).toContain('c.png')
  })

  it('says when they cannot be saved', async () => {
    on('PUT', '/api/v1/files/favourites', () => problem(422, 'invalid_operation'))
    await show()
    await press('Add this folder to the favourites')
    expect(useNotifications().items.some((n) => n.kind === 'error')).toBe(true)
  })
})

describe('snapshots', () => {
  it('looks into one read only and copies entries back', async () => {
    answer('GET', '/api/v1/files/shares/1/snapshots', [
      { label: 'zdl-2026-11-07_10.00.00', taken_at: 1_793_000_000, note: 'Before' },
    ])
    on('POST', '/api/v1/files/shares/1/snapshots/zdl-2026-11-07_10.00.00/restore', () =>
      job(8, 'files.restore'),
    )
    await show()

    await choose('select[aria-label="Point in time"]', 'zdl-2026-11-07_10.00.00')
    expect(document.body.querySelector('[role="status"]')!.textContent).toContain('As it was on')
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').at(-1)!.query.get('snapshot')).toBe(
      'zdl-2026-11-07_10.00.00',
    )
    expect(button('Upload')!.disabled).toBe(true)
    await select('b.txt')
    await click('Copy back')
    expect(callsTo('POST', /snapshots\/.*\/restore/)[0]!.body).toEqual({ paths: ['b.txt'] })
    expect(notified('Copying 1 item back')).toBe(true)
    await click('Back to now')
    expect(document.body.querySelector('[role="status"]')).toBeNull()
  })

  it('says when copying back fails', async () => {
    answer('GET', '/api/v1/files/shares/1/snapshots', [
      { label: 'zdl-1', taken_at: 1_793_000_000, note: null },
    ])
    on('POST', /snapshots\/.*\/restore/, () => problem(500, 'internal'))
    await show()
    await choose('select[aria-label="Point in time"]', 'zdl-1')
    await select('b.txt')
    await click('Copy back')
    expect(useNotifications().items.some((n) => n.kind === 'error')).toBe(true)
  })
})

describe('uploads', () => {
  function pickFiles(...names: string[]): Promise<void> {
    const picker = document.body.querySelector<HTMLInputElement>('input[type="file"]')!
    Object.defineProperty(picker, 'files', {
      configurable: true,
      value: names.map((name) => new File(['content'], name)),
    })
    picker.dispatchEvent(new Event('change'))
    return settle()
  }

  it('sends new files with their progress, and reads the folder again', async () => {
    await show()
    await pickFiles('new.txt')

    const upload = FakeUpload.sent[0]!
    expect(upload.url).toBe('/api/v1/files/shares/1/upload?path=&name=new.txt&replace=false')
    upload.progress(3, 4)
    await settle()
    expect(document.body.querySelector('[aria-label="Uploading new.txt"]')).not.toBeNull()
    const before = callsTo('GET', '/api/v1/files/shares/1/folder').length
    upload.answer(201, '{"path":"new.txt"}')
    await settle()
    expect(document.body.querySelector('[aria-label="Uploading new.txt"]')).toBeNull()
    expect(callsTo('GET', '/api/v1/files/shares/1/folder').length).toBeGreaterThan(before)
  })

  it('asks before replacing, and skips taken names when told', async () => {
    await show()
    await pickFiles('b.txt', 'other.txt')
    expect(page()).toContain('This folder already contains: b.txt.')
    await click('Replace')
    expect(FakeUpload.sent.map((u) => u.url.split('?')[1])).toEqual([
      'path=&name=b.txt&replace=true',
      'path=&name=other.txt&replace=false',
    ])

    FakeUpload.sent = []
    await pickFiles('b.txt')
    await click('Skip')
    expect(FakeUpload.sent).toHaveLength(0)
  })

  it('shows a failed upload until dismissed, and forgets a cancelled one', async () => {
    await show()
    await pickFiles('big.iso', 'small.txt')
    FakeUpload.sent[0]!.answer(507, '{"code":"internal"}')
    await settle()
    expect(button('')).toBeDefined()
    await press('Dismiss')
    expect(page()).not.toContain('big.iso')
    await press('Cancel the upload')
    expect(page()).not.toContain('small.txt')
  })

  it('takes files dropped on the folder', async () => {
    await show()
    const area = document.body.querySelector<HTMLElement>('[aria-busy]')!.parentElement!
    area.dispatchEvent(new Event('dragover', { bubbles: true, cancelable: true }))
    await settle()
    expect(page()).toContain('Drop files here to upload them')
    area.dispatchEvent(new Event('dragleave', { bubbles: true }))
    await settle()
    const drop = new Event('drop', { bubbles: true, cancelable: true })
    Object.defineProperty(drop, 'dataTransfer', {
      value: { files: [new File(['x'], 'dropped.txt')] },
    })
    area.dispatchEvent(drop)
    await settle()
    expect(FakeUpload.sent[0]!.url).toContain('name=dropped.txt')
  })
})

describe('previews', () => {
  it('shows a picture, and the beginning of a text', async () => {
    on('GET', '/api/v1/files/shares/1/download', ({ query }) =>
      query.get('path') === 'b.txt'
        ? new Response('hello', {
            status: 206,
            headers: { 'Content-Range': `bytes 0-4/${10 ** 9}` },
          })
        : problem(404, 'not_found'),
    )
    await show()

    document.body.querySelector<HTMLButtonElement>('button[title="Show"]')!.click()
    await settle()
    expect(document.body.querySelector<HTMLImageElement>('img[alt="a.jpg"]')!.src).toContain(
      'inline=true',
    )
    document.body
      .querySelector<HTMLButtonElement>('[role="dialog"] button[aria-label="Close"]')!
      .click()
    await settle()

    const text = [
      ...document.body.querySelectorAll<HTMLButtonElement>('button[title="Show"]'),
    ].find((b) => b.textContent?.includes('b.txt'))!
    text.click()
    await settle()
    expect(document.body.querySelector('pre')!.textContent).toBe('hello')
    expect(page()).toContain('Only the beginning is shown.')
  })

  it('says when a text cannot be read', async () => {
    on('GET', '/api/v1/files/shares/1/download', () => problem(403, 'permission_denied'))
    await show()
    const text = [
      ...document.body.querySelectorAll<HTMLButtonElement>('button[title="Show"]'),
    ].find((b) => b.textContent?.includes('b.txt'))!
    text.click()
    await settle()
    expect(document.body.querySelector('[role="dialog"] [role="alert"]')).not.toBeNull()
  })
})
