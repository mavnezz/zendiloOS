import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { Account, ChannelInput, NotificationChannel } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import ChannelDialog from './ChannelDialog.vue'

const create = vi.fn<(body: ChannelInput) => Promise<void>>(() => Promise.resolve())
const update = vi.fn<(id: number, body: ChannelInput) => Promise<void>>(() => Promise.resolve())

const account = (id: number, name: string, more: Partial<Account> = {}): Account => ({
  id,
  name,
  role: 'user',
  disabled: false,
  created_at: 0,
  updated_at: 0,
  password_changed_at: 0,
  password_change_required: false,
  password_locked: false,
  two_factor: false,
  ...more,
})

const accounts = ref<Account[] | undefined>()
const accountsError = ref<Error | null>(null)

vi.mock('../api', () => ({
  useAccounts: () => ({ data: accounts, error: accountsError }),
  useChannelActions: () => ({
    create: (body: ChannelInput) => create(body),
    update: (id: number, body: ChannelInput) => update(id, body),
  }),
}))

const stored: NotificationChannel = {
  id: 4,
  name: 'Admins',
  target: { kind: 'email', accounts: [2, 7], to: ['a@example.org', 'b@example.org'] },
  recipients: [],
  language: 'de',
  last_delivery_at: null,
  last_error: null,
  waiting: [],
  failed: [],
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

function accountBox(text: string): HTMLInputElement {
  const label = [...document.body.querySelectorAll('label')].find((candidate) =>
    candidate.textContent?.includes(text),
  )
  const box = label?.querySelector<HTMLInputElement>('input[type="checkbox"]')
  if (!box) throw new Error(`no account ${text}`)
  return box
}

async function submit(): Promise<void> {
  document.body.querySelector('form')?.dispatchEvent(new Event('submit'))
  await flushPromises()
}

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

beforeEach(() => {
  i18n.global.locale.value = 'en'
  accounts.value = [
    account(2, 'anna', { email: 'anna@example.org', full_name: 'Anna Berg' }),
    account(3, 'ben'),
    account(5, 'carl', { email: 'carl@example.org', disabled: true }),
  ]
  accountsError.value = null
  create.mockClear()
  update.mockClear()
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the channel dialog', () => {
  it('adds an email channel to accounts and addresses', async () => {
    const wrapper = await mountDialog(null)
    expect(document.body.textContent).toContain('Anna Berg (anna)')
    expect(document.body.textContent).toContain(t('channels.problems.no_address'))
    expect(document.body.textContent).toContain(t('channels.problems.disabled'))
    await type('channel-name', 'Family')
    accountBox('anna').click()
    await flushPromises()
    accountBox('ben').click()
    await flushPromises()
    await type('channel-to', 'a@example.org, b@example.org')
    await submit()

    expect(create).toHaveBeenCalledWith({
      name: 'Family',
      target: { kind: 'email', accounts: [2, 3], to: ['a@example.org', 'b@example.org'] },
      language: 'en',
    })
    wrapper.unmount()
  })

  it('asks for a recipient before saving', async () => {
    const wrapper = await mountDialog(null)
    await type('channel-name', 'Nobody')
    await submit()
    expect(create).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain(t('channels.noRecipient'))
    wrapper.unmount()
  })

  it('adds a webhook', async () => {
    const wrapper = await mountDialog(null)
    await type('channel-name', 'Chat')
    await type('channel-kind', 'webhook')
    await type('channel-url', 'https://ntfy.example.org/nas')
    await type('channel-format', 'text')
    await type('channel-language', 'de')
    await submit()
    expect(create).toHaveBeenCalledWith({
      name: 'Chat',
      target: { kind: 'webhook', url: 'https://ntfy.example.org/nas', format: 'text' },
      language: 'de',
    })
    wrapper.unmount()
  })

  it('changes a channel and lets a deleted account be taken out', async () => {
    const wrapper = await mountDialog(stored)
    expect((field('channel-to') as HTMLInputElement).value).toBe('a@example.org, b@example.org')
    expect(accountBox('anna').checked).toBe(true)
    const gone = accountBox(t('channels.account', { id: 7 }))
    expect(gone.checked).toBe(true)
    expect(document.body.textContent).toContain(t('channels.problems.gone'))
    gone.click()
    await type('channel-name', 'Household')
    await submit()

    expect(update).toHaveBeenCalledWith(4, {
      name: 'Household',
      target: { kind: 'email', accounts: [2], to: ['a@example.org', 'b@example.org'] },
      language: 'de',
    })
    wrapper.unmount()
  })

  it('says why it failed, and when the accounts cannot be listed', async () => {
    accounts.value = undefined
    accountsError.value = new Error('forbidden')
    update.mockRejectedValueOnce(new Error('no'))
    const wrapper = await mountDialog({
      ...stored,
      target: { kind: 'webhook', url: 'https://hooks.example.org', format: 'json' },
    })
    expect((field('channel-url') as HTMLInputElement).value).toBe('https://hooks.example.org')
    await type('channel-kind', 'email')
    expect(document.body.textContent).toContain(t('channels.accountsUnknown'))
    await type('channel-kind', 'webhook')
    await submit()
    expect(document.body.textContent).toContain(t('errors.unknown', { code: 'client' }))
    wrapper.unmount()
  })
})
