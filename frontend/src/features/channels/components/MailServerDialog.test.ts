import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { MailServerInput, MailServerSettings } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import MailServerDialog from './MailServerDialog.vue'

const saveMailServer = vi.fn<(body: MailServerInput) => Promise<void>>(() => Promise.resolve())

vi.mock('../api', () => ({
  useChannelActions: () => ({ saveMailServer: (body: MailServerInput) => saveMailServer(body) }),
}))

const stored: MailServerSettings = {
  server: 'smtp.example.org',
  port: 587,
  security: 'starttls',
  username: 'nas',
  from: 'nas@example.org',
  password_set: true,
}

async function mountDialog(server: MailServerSettings | null) {
  const wrapper = mount(MailServerDialog, {
    props: { open: false, server },
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
  saveMailServer.mockClear()
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the mail server dialog', () => {
  it('sets up a server with its password, the port following the encryption', async () => {
    const wrapper = await mountDialog(null)
    await type('mail-server', 'smtp.example.org')
    await type('mail-security', 'tls')
    await type('mail-username', 'nas')
    await type('mail-password', 'secret')
    await type('mail-from', 'nas@example.org')
    await submit()

    expect(saveMailServer).toHaveBeenCalledWith({
      server: 'smtp.example.org',
      port: 465,
      security: 'tls',
      username: 'nas',
      from: 'nas@example.org',
      password: 'secret',
    })
    wrapper.unmount()
  })

  it('keeps a stored password unless a new one is typed', async () => {
    const wrapper = await mountDialog(stored)
    expect((field('mail-server') as HTMLInputElement).value).toBe('smtp.example.org')
    expect((field('mail-password') as HTMLInputElement).placeholder).toBe(
      i18n.global.t('channels.mailServer.passwordKept'),
    )
    await type('mail-from', 'alerts@example.org')
    await submit()
    const body = saveMailServer.mock.calls[0]?.[0]
    expect(body?.from).toBe('alerts@example.org')
    expect(body && 'password' in body).toBe(false)
    wrapper.unmount()
  })

  it('removes a stored password, and keeps a port typed by hand', async () => {
    const wrapper = await mountDialog(stored)
    const clear = document.body.querySelector<HTMLInputElement>('input[type="checkbox"]')!
    clear.click()
    await flushPromises()
    await type('mail-port', '2525')
    await type('mail-security', 'none')
    await type('mail-username', '')
    await submit()
    expect(saveMailServer).toHaveBeenCalledWith(
      expect.objectContaining({ port: 2525, security: 'none', username: null, password: '' }),
    )
    wrapper.unmount()
  })

  it('says why it was not saved', async () => {
    saveMailServer.mockRejectedValueOnce(new Error('no'))
    const wrapper = await mountDialog(stored)
    await submit()
    expect(document.body.textContent).toContain(i18n.global.t('errors.unknown', { code: 'client' }))
    wrapper.unmount()
  })
})
