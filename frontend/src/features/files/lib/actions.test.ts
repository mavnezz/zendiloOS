import { describe, expect, it } from 'vitest'

import {
  actionsFor,
  dropMode,
  extension,
  type Item,
  type Situation,
  shortcut,
  sortItems,
} from './actions'
import {
  folderAt,
  inBin,
  ownBin,
  parentPath,
  sameLocation,
  useHistory,
  withinBin,
} from './location'

function item(name: string, extra: Partial<Item> = {}): Item {
  return {
    key: name,
    name,
    kind: 'file',
    size: 1,
    modified: 1,
    unreadable: false,
    share: 1,
    path: name,
    ...extra,
  }
}

function situation(extra: Partial<Situation> = {}): Situation {
  return {
    kind: 'folder',
    searching: false,
    writable: true,
    snapshot: false,
    ownBin: false,
    binToEmpty: false,
    shareWritable: true,
    canManage: false,
    clipboard: false,
    favourite: false,
    favouriteTarget: true,
    hasSnapshots: true,
    shareTarget: null,
    drive: null,
    items: [],
    ...extra,
  }
}

const ids = (s: Situation) => actionsFor(s).map((a) => a.id)
const enabled = (s: Situation) =>
  actionsFor(s)
    .filter((a) => !a.disabled)
    .map((a) => a.id)

describe('the actions', () => {
  it('belong where the explorer is', () => {
    expect(ids(situation())).not.toContain('restore')
    expect(ids(situation({ kind: 'bin' }))).toEqual(
      expect.arrayContaining(['restore', 'emptyBin', 'delete']),
    )
    expect(ids(situation({ kind: 'bin' }))).not.toContain('copy')
    expect(ids(situation({ snapshot: true }))).toContain('copyBack')
    expect(ids(situation({ snapshot: true }))).not.toContain('paste')
    expect(ids(situation({ kind: 'volume' }))).not.toContain('newFolder')
    expect(ids(situation({ kind: 'volume', canManage: true }))).toContain('newFolder')
    expect(ids(situation({ canManage: true, shareTarget: { shared: true } }))).toContain('share')
    expect(ids(situation())).not.toContain('openLocation')
    expect(ids(situation({ searching: true }))).toContain('openLocation')
    expect(enabled(situation({ searching: true }))).not.toContain('openLocation')
    expect(enabled(situation({ searching: true, items: [item('a/b.txt')] }))).toContain(
      'openLocation',
    )
  })

  it('are enabled for what is chosen', () => {
    expect(enabled(situation())).toEqual(
      expect.arrayContaining(['newFolder', 'upload', 'favourite', 'properties']),
    )
    expect(enabled(situation())).not.toContain('paste')
    expect(enabled(situation({ clipboard: true }))).toContain('paste')
    const one = situation({ items: [item('a.txt')] })
    expect(enabled(one)).toEqual(expect.arrayContaining(['open', 'download', 'rename', 'delete']))
    const two = situation({ items: [item('a'), item('b')] })
    expect(enabled(two)).not.toContain('rename')
    expect(enabled(two)).not.toContain('download')
    const read = situation({ writable: false, items: [item('a')] })
    expect(enabled(read)).toEqual(expect.arrayContaining(['copy', 'open']))
    expect(enabled(read)).not.toContain('cut')
  })

  it('treat a folder on a volume as such', () => {
    const folder = item('media', {
      kind: 'directory',
      path: '',
      volumeFolder: { shared: false, access: null },
    })
    const volume = situation({ kind: 'volume', items: [folder], shareTarget: { shared: false } })
    expect(enabled(volume)).not.toContain('open')
    expect(enabled(volume)).not.toContain('delete')
    expect(enabled({ ...volume, canManage: true })).toEqual(
      expect.arrayContaining(['delete', 'share']),
    )
    expect(enabled({ ...volume, canManage: true })).not.toContain('copy')
    const shared = actionsFor({ ...volume, canManage: true, shareTarget: { shared: true } })
    expect(shared.find((a) => a.id === 'share')!.undo).toBe(true)
  })

  it('treat a removable drive as such', () => {
    // Inside a mounted drive: eject, no favourites, no snapshots.
    const inside = situation({ drive: 'mounted', items: [item('a.txt', { share: 'STICK' })] })
    expect(enabled(inside)).toEqual(expect.arrayContaining(['eject', 'copy', 'delete', 'rename']))
    expect(ids(inside)).not.toEqual(expect.arrayContaining(['favourite']))
    expect(ids(inside)).not.toContain('previousVersions')
    expect(ids(inside)).not.toContain('mount')
    // The drive itself, in the tree: opened, ejected; never copied or deleted.
    const node = item('STICK', {
      kind: 'directory',
      share: 'STICK',
      path: '',
      drive: { state: 'mounted', problem: null },
    })
    const mounted = situation({ drive: 'mounted', items: [node] })
    expect(enabled(mounted)).toEqual(expect.arrayContaining(['open', 'eject', 'properties']))
    for (const id of ['copy', 'cut', 'rename', 'delete', 'download'] as const) {
      expect(enabled(mounted)).not.toContain(id)
    }
    // Ejected: mounted again, nothing else.
    const ejected = situation({
      drive: 'ejected',
      writable: false,
      items: [{ ...node, drive: { state: 'ejected', problem: null } }],
    })
    expect(enabled(ejected)).toContain('mount')
    expect(enabled(ejected)).not.toContain('open')
    expect(enabled(ejected)).not.toContain('eject')
    expect(enabled(ejected)).not.toContain('properties')
  })

  it('only open the personal folders listed, and nothing in another’s', () => {
    const others = item('bob', {
      kind: 'directory',
      share: 0,
      path: '',
      personal: { own: false, volume: 1 },
    })
    const homes = situation({ kind: 'homes', writable: false, canManage: true, items: [others] })
    expect(ids(homes)).toEqual(['open'])
    expect(enabled(homes)).toEqual(['open'])
    expect(ids(situation({ kind: 'private', canManage: true }))).toEqual([])
  })

  it('know their keys', () => {
    const press = (key: string, init: KeyboardEventInit = {}) =>
      shortcut(new KeyboardEvent('keydown', { key, ...init }))
    expect(press('Delete')).toBe('delete')
    expect(press('F2')).toBe('rename')
    expect(press('Enter')).toBe('open')
    expect(press('Backspace')).toBe('up')
    expect(press('ArrowUp', { altKey: true })).toBe('up')
    expect(press('ArrowRight', { altKey: true })).toBe('forward')
    expect(press('c', { metaKey: true })).toBe('copy')
    expect(press('q', { ctrlKey: true })).toBeNull()
    expect(press('x')).toBeNull()
  })

  it('move when dropped within one place and copy into another, unless a key says otherwise', () => {
    const keys = (init: MouseEventInit = {}) => new MouseEvent('drop', init)
    expect(dropMode(1, 1, keys())).toBe('move')
    expect(dropMode(1, 2, keys())).toBe('copy')
    expect(dropMode(1, '1A2B-3C4D', keys())).toBe('copy')
    expect(dropMode('1A2B-3C4D', '1A2B-3C4D', keys())).toBe('move')
    expect(dropMode(0, 1, keys())).toBe('copy')
    expect(dropMode(1, 1, keys({ ctrlKey: true }))).toBe('copy')
    expect(dropMode(1, 1, keys({ altKey: true }))).toBe('copy')
    expect(dropMode(1, 2, keys({ shiftKey: true }))).toBe('move')
  })
})

describe('entries', () => {
  it('are sorted folders first, by name, date, type or size', () => {
    const list = [
      item('b.txt', { size: 3, modified: 1 }),
      item('A', { kind: 'directory', size: null }),
      item('a10.pdf', { size: 1, modified: 3 }),
      item('a9.pdf', { size: 2, modified: 2 }),
    ]
    const names = (key: Parameters<typeof sortItems>[1], descending = false) =>
      sortItems(list, key, descending).map((i) => i.name)
    expect(names('name')).toEqual(['A', 'a9.pdf', 'a10.pdf', 'b.txt'])
    expect(names('name', true)).toEqual(['A', 'b.txt', 'a10.pdf', 'a9.pdf'])
    expect(names('modified')).toEqual(['A', 'b.txt', 'a9.pdf', 'a10.pdf'])
    expect(names('size')).toEqual(['A', 'a10.pdf', 'a9.pdf', 'b.txt'])
    expect(names('type')).toEqual(['A', 'a9.pdf', 'a10.pdf', 'b.txt'])
    expect(extension('archive.tar.gz')).toBe('GZ')
    expect(extension('.profile')).toBeNull()
    expect(extension('dot.')).toBeNull()
  })
})

describe('locations', () => {
  it('compare, nest and go back and forth', () => {
    expect(sameLocation(folderAt(1, 'a'), folderAt(1, 'a'))).toBe(true)
    expect(sameLocation(folderAt(1, 'a'), folderAt(1, 'a', 'snap'))).toBe(false)
    expect(sameLocation({ kind: 'volume', volume: 1 }, { kind: 'volume', volume: 2 })).toBe(false)
    expect(sameLocation({ kind: 'bin' }, { kind: 'bin' })).toBe(true)
    expect(sameLocation({ kind: 'bin' }, { kind: 'bin', share: 1, path: '' })).toBe(false)
    expect(sameLocation({ kind: 'bin', share: 1 }, { kind: 'bin', share: 1, path: '' })).toBe(true)
    expect(sameLocation({ kind: 'bin', share: 1, path: 'a' }, { kind: 'bin', share: 1 })).toBe(
      false,
    )
    expect(sameLocation({ kind: 'homes', volume: 1 }, { kind: 'homes', volume: 1 })).toBe(true)
    expect(sameLocation({ kind: 'homes', volume: 1 }, { kind: 'volume', volume: 1 })).toBe(false)
    const bob = { kind: 'private', volume: 1, name: 'bob' } as const
    expect(sameLocation(bob, { ...bob })).toBe(true)
    expect(sameLocation(bob, { ...bob, name: 'carol' })).toBe(false)
    expect(withinBin('.recycle/anna', '.recycle/anna/a/b')).toBe('a/b')
    expect(withinBin('.recycle/anna', '.recycle/anna')).toBe('')
    expect(withinBin('.recycle/anna', '.recycle/annabel/x')).toBeNull()
    expect(withinBin(null, '.recycle/anna/x')).toBeNull()
    expect(parentPath('a/b')).toBe('a')
    expect(parentPath('a')).toBe('')
    expect(ownBin('anna')).toBe('.recycle/anna')
    expect(ownBin(undefined)).toBeNull()
    expect(inBin('.recycle/anna/x')).toBe(true)

    const history = useHistory({ kind: 'bin' })
    history.go(folderAt(1))
    history.go(folderAt(1))
    history.go(folderAt(2))
    history.back()
    expect(history.current.value).toEqual(folderAt(1))
    history.go(folderAt(3))
    expect(history.canForward.value).toBe(false)
    history.back()
    history.back()
    history.back()
    expect(history.canBack.value).toBe(false)
    history.replace(folderAt(9))
    history.forward()
    history.forward()
    history.forward()
    expect(history.current.value).toEqual(folderAt(3))
  })
})
