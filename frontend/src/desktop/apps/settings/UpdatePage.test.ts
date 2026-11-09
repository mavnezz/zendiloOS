import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { button, click, mountApp, page, settle } from '@/desktop/testing/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { UpdatesOverview } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { Markdown } from '@/shared/ui/markdown'

import UpdatePage from './UpdatePage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const request = vi.fn<(action: string) => Promise<void>>(() => Promise.resolve())
vi.mock('@/desktop/power', () => ({
  usePower: () => ({ request: (action: string) => request(action) }),
}))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function overview(extra: Partial<UpdatesOverview> = {}): UpdatesOverview {
  return {
    current: '0.1.1',
    current_notes: null,
    checked_at: 1_790_000_000,
    phase: 'idle',
    phase_error: null,
    packages: [
      { name: 'linux-image-amd64', from: '6.12.1', to: '6.12.2', security: true },
      { name: 'curl', from: '8.1', to: '8.2', security: false },
    ],
    packages_error: null,
    release: {
      version: '0.2.0',
      tag: 'v0.2.0',
      channel: 'test',
      notes: '## New\n\n- Faster shares',
      published_at: 1_790_000_000,
      size: 30_000_000,
    },
    release_error: null,
    restart_needed: false,
    run: null,
    running_kernel: '6.12.1',
    newest_kernel: '6.12.2',
    settings: { channel: 'stable', check_daily: true },
    zfs_ready: true,
    interrupted: false,
    ...extra,
  }
}

function confirm(wrapper: VueWrapper): void {
  wrapper.findComponent({ name: 'ConfirmDialog' }).vm.$emit('confirm')
}

beforeEach(() => {
  reset()
  request.mockClear()
})

describe('the update page', () => {
  it('shows what is available and installs it once confirmed', async () => {
    on('GET', '/api/v1/system/updates', overview())
    on('POST', '/api/v1/system/updates/apply', null)
    on('POST', '/api/v1/system/updates/check', null)
    const { wrapper } = await mountApp(UpdatePage)

    expect(page()).toContain('0.2.0')
    expect(page()).toContain(t('updates.channels.test'))
    await click(new RegExp(t('updates.showNotes')))
    expect(page()).toContain('Faster shares')
    await click(new RegExp(t('updates.showPackages')))
    expect(page()).toContain('curl')
    await click(new RegExp(t('updates.check')))
    expect(called('POST', '/api/v1/system/updates/check')).toHaveLength(1)

    await click(new RegExp(t('updates.updateNow')))
    confirm(wrapper)
    await settle()
    expect(called('POST', '/api/v1/system/updates/apply')).toHaveLength(1)
  })

  it('shows what is new in the installed version, as Markdown, with HTML as text', async () => {
    on(
      'GET',
      '/api/v1/system/updates',
      overview({
        current_notes:
          'Changes since v0.1.0:\n\n- **Faster** shares\n- <img src=x onerror="alert(1)"> fixed',
      }),
    )
    const { wrapper } = await mountApp(UpdatePage)
    expect(page()).not.toContain('Faster')

    await click(new RegExp(t('updates.currentNotes')))
    expect(button(new RegExp(t('updates.currentNotes'))).getAttribute('aria-expanded')).toBe('true')
    const notes = wrapper.findComponent(Markdown)
    expect(notes.find('strong').text()).toBe('Faster')
    expect(notes.find('img').exists()).toBe(false)
    expect(notes.text()).toContain('<img src=x onerror="alert(1)"> fixed')

    await click(new RegExp(t('updates.currentNotes')))
    expect(page()).not.toContain('Faster')
  })

  it('renders the release notes as Markdown, with HTML as text', async () => {
    on(
      'GET',
      '/api/v1/system/updates',
      overview({
        release: {
          ...overview().release!,
          notes: '## New\n\n- <b>Bold</b> claims, *quietly*\n\n<script>alert(1)</script>',
        },
      }),
    )
    const { wrapper } = await mountApp(UpdatePage)
    expect(page(), 'a package without notes').not.toContain(t('updates.currentNotes'))

    await click(new RegExp(t('updates.showNotes')))
    const notes = wrapper.findComponent(Markdown)
    expect(notes.find('h2').text()).toBe('New')
    expect(notes.find('em').text()).toBe('quietly')
    expect(notes.find('b').exists()).toBe(false)
    expect(notes.find('script').exists()).toBe(false)
    expect(notes.text()).toContain('<b>Bold</b> claims')
    expect(notes.text()).toContain('<script>alert(1)</script>')
  })

  it('follows a run and offers the restart it needs', async () => {
    on(
      'GET',
      '/api/v1/system/updates',
      overview({
        restart_needed: true,
        run: {
          id: 'run1',
          finishes: false,
          state: 'succeeded',
          step: 'modules',
          started_at: 1_790_000_000,
          finished_at: 1_790_000_600,
          zendiloos: '0.2.0',
          restart_needed: true,
          zfs_ready: true,
          error: null,
        },
        packages: [],
        release: null,
      }),
    )
    const { wrapper } = await mountApp(UpdatePage)
    expect(page()).toContain(t('updates.runTitle'))
    await click(new RegExp(t('updates.restartNow')))
    confirm(wrapper)
    await settle()
    expect(request).toHaveBeenCalledWith('reboot')
  })

  it('says what went wrong, and that the ZFS module is not ready', async () => {
    on(
      'GET',
      '/api/v1/system/updates',
      overview({
        phase: 'failed',
        phase_error: 'download failed',
        release_error: 'GitHub unreachable',
        packages_error: 'apt failed',
        run: {
          id: 'run2',
          finishes: false,
          state: 'failed',
          step: 'debian',
          started_at: 1_790_000_000,
          finished_at: 1_790_000_600,
          zendiloos: null,
          restart_needed: false,
          zfs_ready: false,
          error: 'dpkg was interrupted',
        },
      }),
    )
    on('POST', '/api/v1/system/updates/apply', problem(409, 'jobs_running', 'files.copy photos'))
    const { wrapper } = await mountApp(UpdatePage)
    expect(page()).toContain('download failed')
    expect(page()).toContain('dpkg was interrupted')
    expect(page()).toContain('GitHub unreachable')
    await click(new RegExp(t('updates.updateNow')))
    confirm(wrapper)
    await settle()
    expect(page()).toContain('files.copy photos')
  })

  it('offers to finish an update dpkg did not finish, and follows it', async () => {
    let finishing = false
    on('GET', '/api/v1/system/updates', () =>
      overview({
        interrupted: !finishing,
        packages: [],
        release: null,
        run: {
          id: finishing ? 'run5' : 'run4',
          finishes: finishing,
          state: finishing ? 'running' : 'failed',
          step: finishing ? 'repair' : 'debian',
          started_at: 1_790_000_000,
          finished_at: null,
          zendiloos: null,
          restart_needed: false,
          zfs_ready: true,
          error: finishing ? null : 'the run stopped before it finished (was the NAS restarted?)',
        },
      }),
    )
    on('POST', '/api/v1/system/updates/finish', () => {
      finishing = true
      return null
    })
    const { wrapper } = await mountApp(UpdatePage)
    expect(page()).toContain(t('updates.interrupted'))
    expect(page(), 'offered once, where dpkg says so').not.toContain(t('updates.finishHint'))
    await click(new RegExp(t('updates.finishTitle')))
    expect(wrapper.findComponent({ name: 'ConfirmDialog' }).props('title')).toBe(
      t('updates.confirmFinishTitle'),
    )
    confirm(wrapper)
    await settle()
    expect(called('POST', '/api/v1/system/updates/finish')).toHaveLength(1)
    expect(page()).not.toContain(t('updates.interrupted'))
    expect(page()).toContain(t('updates.steps.configure'))
    expect(page()).toContain(t('updates.steps.repair'))
    expect(page()).not.toContain(t('updates.steps.lists'))
  })

  it('offers finishing after an update that failed, also before a look found why', async () => {
    on(
      'GET',
      '/api/v1/system/updates',
      overview({
        run: {
          id: 'run6',
          finishes: false,
          state: 'failed',
          step: 'modules',
          started_at: 1_790_000_000,
          finished_at: 1_790_000_600,
          zendiloos: null,
          restart_needed: false,
          zfs_ready: true,
          error: 'apt-get upgrade failed',
        },
      }),
    )
    await mountApp(UpdatePage)
    expect(page()).toContain(t('updates.finishHint'))
    expect(page()).not.toContain(t('updates.interrupted'))
  })

  it('shows a run going on, and a newer version than the page has', async () => {
    let current = '0.1.1'
    on('GET', '/api/v1/system/updates', () =>
      overview({
        current,
        run: {
          id: 'run3',
          finishes: false,
          state: 'running',
          step: 'zendiloos',
          started_at: 1_790_000_000,
          finished_at: null,
          zendiloos: '0.2.0',
          restart_needed: false,
          zfs_ready: true,
          error: null,
        },
      }),
    )
    const { queryClient } = await mountApp(UpdatePage)
    expect(page()).toContain(t('updates.runHint'))
    current = '0.2.0'
    await queryClient.invalidateQueries()
    await settle()
    expect(page()).toContain(t('updates.reload'))
  })

  it('keeps the channel and the daily check', async () => {
    on('GET', '/api/v1/system/updates', overview())
    on('PUT', '/api/v1/system/updates/settings', (call: { body: unknown }) => call.body)
    const { wrapper } = await mountApp(UpdatePage)
    wrapper.findComponent({ name: 'PageTabs' }).vm.$emit('update:modelValue', 'settings')
    await settle()
    wrapper.findComponent({ name: 'RadioGroup' }).vm.$emit('update:modelValue', 'development')
    await settle()
    expect(page()).toContain(t('updates.developmentWarning'))
    wrapper.findComponent({ name: 'PageFooter' }).vm.$emit('apply')
    await settle()
    expect(called('PUT', '/api/v1/system/updates/settings')[0]?.body).toEqual({
      channel: 'development',
      check_daily: true,
    })

    on('PUT', '/api/v1/system/updates/settings', problem(422, 'invalid_input', 'no such channel'))
    wrapper.findComponent({ name: 'RadioGroup' }).vm.$emit('update:modelValue', 'test')
    await settle()
    wrapper.findComponent({ name: 'PageFooter' }).vm.$emit('apply')
    await settle()
    expect(page()).toContain('no such channel')
    wrapper.findComponent({ name: 'PageFooter' }).vm.$emit('reset')
    await settle()
    expect(wrapper.findComponent({ name: 'RadioGroup' }).props('modelValue')).toBe('stable')
  })
})
