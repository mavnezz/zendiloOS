import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mountApp, settle } from '@/desktop/testing/mount'
import { useSessionStore } from '@/features/auth/store'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { useNotifications } from '@/shared/lib/notifications'
import { DELAY_MS, PROJECT_URL, RETRY_MS } from '@/shared/lib/star-prompt'
import { usePreferences } from '@/shared/preferences/store'
import { called, on, reset } from '@/shared/test-support/server'
import { job } from '@/shared/testing/fixtures'

import StarPrompt from './StarPrompt.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({
  live: (await import('@/shared/test-support/live')).live,
}))

const t = (key: string) => i18n.global.t(key)
const DAY = 86_400
/** As `signIn` creates the account. */
const CREATED = 1_700_000_000

/** The card, if it is there. */
const card = () =>
  document.querySelector<HTMLElement>(`section[aria-label="${t('desktop.starPrompt.label')}"]`)
const saved = () => called('PUT', '/api/v1/preferences').map((call) => call.body)

function overview(verdict: string | null): void {
  on('GET', '/api/v1/storage/overview', {
    pools: 1,
    disks: 2,
    used_bytes: 1,
    available_bytes: 1,
    verdict,
    concerns: [],
  })
}

/** Mounted on a desktop whose account is `days` old and whose choices arrived. */
async function desktop(days = 20): Promise<void> {
  vi.setSystemTime((CREATED + days * DAY) * 1000)
  await mountApp(StarPrompt)
  await usePreferences().load()
}

/** Lets `ms` go by, and the page follow. */
async function wait(ms: number): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms)
  await settle()
}

beforeEach(() => {
  reset()
  sessionStorage.clear()
  vi.useFakeTimers({ shouldAdvanceTime: true })
  on('GET', '/api/v1/preferences', { language: null, theme: null })
  on('PUT', '/api/v1/preferences', null)
  overview(null)
})

afterEach(() => {
  vi.useRealTimers()
})

describe('the reminder to star zendiloOS', () => {
  it('appears a while after the desktop, as a card with a link', async () => {
    await desktop()
    await wait(DELAY_MS / 2)
    expect(card()).toBeNull()
    expect(called('GET', '/api/v1/storage/overview'), 'not asked before').toHaveLength(0)
    await wait(DELAY_MS / 2)
    // The storage overview was not known yet: it is asked for, then looked at.
    await wait(RETRY_MS)

    expect(card()?.textContent).toContain(t('desktop.starPrompt.text'))
    const shown = saved().at(-1) as { star_prompt: { last_shown_at: number } }
    expect(shown).toMatchObject({
      star_prompt: { times_shown: 1, last_shown_at: expect.any(Number) as number, answer: null },
    })
    expect(shown.star_prompt.last_shown_at).toBeGreaterThanOrEqual(CREATED + 20 * DAY)

    const link = card()!.querySelector('a')!
    expect([link.href, link.target, link.rel]).toEqual([
      PROJECT_URL,
      '_blank',
      'noopener noreferrer',
    ])
    expect(link.textContent?.trim()).toBe(t('desktop.starPrompt.view'))
    link.addEventListener('click', (event) => event.preventDefault())
    link.click()
    await settle()
    expect(card()).toBeNull()
    expect(saved().at(-1)).toMatchObject({ star_prompt: { times_shown: 1, answer: 'starred' } })
  })

  it('takes Escape and its close button for “Later”, and comes once more', async () => {
    await desktop()
    await wait(DELAY_MS + RETRY_MS)
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    await settle()
    expect(card()).toBeNull()
    expect(saved().at(-1)).toMatchObject({ star_prompt: { answer: 'later' } })

    // Other sessions, some time later.
    sessionStorage.clear()
    on('GET', '/api/v1/preferences', {
      language: null,
      theme: null,
      star_prompt: { times_shown: 1, last_shown_at: CREATED + 20 * DAY, answer: 'later' },
    })
    await desktop(20 + 59)
    await wait(DELAY_MS + 2 * RETRY_MS)
    expect(card(), 'not two months yet').toBeNull()

    await desktop(20 + 61)
    await wait(DELAY_MS + RETRY_MS)
    card()!
      .querySelector<HTMLButtonElement>(`button[aria-label="${t('desktop.close')}"]`)!
      .click()
    await settle()
    expect(card()).toBeNull()
    expect(saved().at(-1)).toMatchObject({ star_prompt: { times_shown: 2, answer: 'later' } })
  })

  it('takes “No thanks” for an answer', async () => {
    await desktop()
    await wait(DELAY_MS + RETRY_MS)
    const decline = [...card()!.querySelectorAll('button')].find(
      (button) => button.textContent?.trim() === t('desktop.starPrompt.decline'),
    )
    decline!.click()
    await settle()
    expect(card()).toBeNull()
    expect(saved().at(-1)).toMatchObject({ star_prompt: { answer: 'declined' } })
  })

  it('leaves Escape to what has the focus', async () => {
    await desktop()
    await wait(DELAY_MS + RETRY_MS)
    const field = document.createElement('input')
    document.body.append(field)
    field.focus()
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await settle()
    expect(card()).not.toBeNull()
  })

  it('waits for a calm moment', async () => {
    overview('warning')
    await desktop()
    const jobs = useJobsStore()
    jobs.update(job(1, 'pool.create'))
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'alertdialog')
    document.body.append(dialog)
    await wait(DELAY_MS + 3 * RETRY_MS)
    expect(card(), 'a job runs, a dialog is open').toBeNull()

    jobs.update(job(1, 'pool.create', { state: 'succeeded' }))
    dialog.remove()
    useNotifications().clear()
    await wait(2 * RETRY_MS)
    expect(card(), 'the storage needs attention').toBeNull()

    useNotifications().notify('error', 'Pool tank is degraded', undefined, { toast: false })
    overview('notice')
    await wait(31_000)
    expect(card(), 'an unread error').toBeNull()

    useNotifications().markAllRead()
    await wait(RETRY_MS)
    expect(card(), 'a notice is no problem').not.toBeNull()
  })

  it('never appears for others, for young accounts, or twice in a session', async () => {
    await desktop(13)
    await wait(DELAY_MS + 2 * RETRY_MS)
    expect(card(), 'two weeks are not over').toBeNull()

    await desktop()
    useSessionStore().info!.account.role = 'user'
    await wait(DELAY_MS + 2 * RETRY_MS)
    expect(card(), 'not an administrator').toBeNull()

    sessionStorage.setItem('zendiloos.starPrompt.shown', '1')
    await desktop()
    await wait(DELAY_MS + 2 * RETRY_MS)
    expect(card(), 'shown in this session').toBeNull()

    sessionStorage.clear()
    on('GET', '/api/v1/preferences', {
      language: null,
      theme: null,
      star_prompt: { times_shown: 2, last_shown_at: CREATED, answer: 'later' },
    })
    await desktop(400)
    await wait(DELAY_MS + 2 * RETRY_MS)
    expect(card(), 'twice is enough').toBeNull()
    expect(called('GET', '/api/v1/storage/overview')).toHaveLength(0)
  })
})
