import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import { i18n } from '@/shared/i18n'

import FolderPicker from './FolderPicker.vue'

const folders: Record<string, string[]> = {
  '': ['Films', 'Music'],
  Films: ['New'],
  'Films/New': [],
}
const listFolders = vi.fn((_share: number, path: string) =>
  Promise.resolve({ names: folders[path] ?? [], writable: true }),
)
const makeFolder = vi.fn((_share: number, path: string) => {
  const parent = path.split('/').slice(0, -1).join('/')
  folders[parent] = [...(folders[parent] ?? []), path.split('/').at(-1)!]
  return Promise.resolve()
})

vi.mock('./folders', () => ({
  useShareList: () => ({ data: ref([{ id: 7, name: 'media' }]) }),
  listFolders: (share: number, path: string) => listFolders(share, path),
  makeFolder: (share: number, path: string) => makeFolder(share, path),
}))

function rows(): string[] {
  return [...document.body.querySelectorAll('[role=treeitem]')].map((row) => row.textContent.trim())
}

function button(text: string): HTMLButtonElement {
  const found = [...document.body.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === text,
  )
  if (!found) throw new Error(`no button ${text}`)
  return found
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  listFolders.mockClear()
  makeFolder.mockClear()
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the folder picker', () => {
  it('opens at the folder picked before and makes new folders there', async () => {
    const wrapper = mount(FolderPicker, {
      props: { open: false, value: 'media/Films/New' },
      global: { plugins: [i18n] },
      attachTo: document.body,
    })
    await wrapper.setProps({ open: true })
    await flushPromises()

    expect(rows()).toEqual(['media', 'Films', 'New', 'Music'])
    expect(listFolders.mock.calls).toEqual([
      [7, ''],
      [7, 'Films'],
    ])
    expect(document.body.querySelector('[aria-selected=true]')?.textContent?.trim()).toBe('New')

    button('New folder').click()
    await flushPromises()
    const name = document.body.querySelector<HTMLInputElement>('#new-folder-name')!
    name.value = 'Season 1'
    name.dispatchEvent(new Event('input'))
    name.form!.dispatchEvent(new Event('submit'))
    await flushPromises()

    expect(makeFolder).toHaveBeenCalledWith(7, 'Films/New/Season 1')
    expect(rows()).toContain('Season 1')
    button('Confirm').click()
    await flushPromises()
    expect(wrapper.emitted('pick')).toEqual([['media/Films/New/Season 1']])
  })
})
