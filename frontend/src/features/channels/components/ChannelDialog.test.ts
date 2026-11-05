import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ChannelInput, NotificationChannel } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ChannelDialog from './ChannelDialog.vue'

const create = vi.fn<(body: ChannelInput) => Promise<void>>(() => Promise.resolve())
const update = vi.fn<(id: number, body: ChannelInput) => Promise<void>>(() => Promise.resolve())

vi.mock('../api', () => ({
  useChannelActions: () => ({
    create: (body: ChannelInput) => create(body),
    update: (id: number, body: ChannelInput) => update(id, body),
  }),
}))

const stored: NotificationChannel = {
  id: 4,
  name: 'Admins',
  target: {
    kind: 'email',
    server: 'smtp.example.org',
    port: 587,
    security: 'starttls',
    username: 'nas',
    from: 'nas@example.org',
    to: ['a@example.org', 'b@example.org'],
  },
  password_set: true,
  min_level: 'warning',
  language: 'de',
  last_delivery_at: null,
  last_error: null,
}

async function mountDialog(channel: NotificationChannel | null) {
  const wrapper = mount(ChannelDialog, {
    props: { open: false, channel },
    global: { plugins: [i18n] },
    attachTo: document.body,
  })
  await wrapper.setProps({ open: true })
  await flushPromises()
  return wrapper
}

function field(id: string): HTMLInputElement | HTMLSelectElement {
  const element = document.body.querySelector<HTMLInputElement | HTMLSelectElement>(`#${id}`)
  if (!element) throw new Error(`no #${id}`)
  return element
}

async function type(id: string, value: string): Promise<void> {
  const element = field(id)
  element.value = value
  element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? 'change' : 'input'))
  await flushPromises()
}

async function submit(): Promise<void> {
  document.body.querySelector('form')?.dispatchEvent(new Event('submit'))
  await flushPromises()
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  create.mockClear()
  update.mockClear()
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the channel dialog', () => {
  it('adds an email channel with its password and recipients', async () => {
    const wrapper = await mountDialog(null)
    await type('channel-name', 'Admins')
    await type('channel-server', 'smtp.example.org')
    await type('channel-security', 'tls')
    await type('channel-username', 'nas')
    await type('channel-password', 'secret')
    await type('channel-from', 'nas@example.org')
    await type('channel-to', 'a@example.org, b@example.org')
    await submit()

    expect(create).toHaveBeenCalledWith({
      name: 'Admins',
      target: {
        kind: 'email',
        server: 'smtp.example.org',
        port: 465,
        security: 'tls',
        username: 'nas',
        from: 'nas@example.org',
        to: ['a@example.org', 'b@example.org'],
      },
      password: 'secret',
      min_level: 'warning',
      language: 'en',
    })
    wrapper.unmount()
  })

  it('keeps a stored password unless a new one is typed', async () => {
    const wrapper = await mountDialog(stored)
    expect((field('channel-to') as HTMLInputElement).value).toBe('a@example.org, b@example.org')
    await type('channel-name', 'Family')
    await submit()

    const body = update.mock.calls[0]?.[1]
    expect(body?.name).toBe('Family')
    expect(body && 'password' in body).toBe(false)
    wrapper.unmount()
  })
})
