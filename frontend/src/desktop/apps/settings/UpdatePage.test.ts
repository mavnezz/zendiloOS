import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { click, mountApp, page, settle } from '@/desktop/testing/mount'
import { called, on, problem, reset } from '@/desktop/testing/server'
import type { UpdatesOverview } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import UpdatePage from './UpdatePage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/desktop/testing/server')).routed(await original()),
)

const request = vi.fn((_action: string) => Promise.resolve())
vi.mock('@/desktop/power', () => ({ usePower: () => ({ request: (action: string) => request(action) }) }))

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function overview(extra: Partial<UpdatesOverview> = {}): UpdatesOverview {
  return {
    current: '0.1.1',
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
    ...extra,
  } as UpdatesOverview
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

  it('follows a run and offers the restart it needs', async () => {
    on('GET', '/api/v1/system/updates', overview({
      restart_needed: true,
      run: {
        id: 'run1',
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
    }))
    const { wrapper } = await mountApp(UpdatePage)
    expect(page()).toContain(t('updates.runTitle'))
    await click(new RegExp(t('updates.restartNow')))
    confirm(wrapper)
    await settle()
    expect(request).toHaveBeenCalledWith('reboot')
  })

  it('says what went wrong, and that the ZFS module is not ready', async () => {
    on('GET', '/api/v1/system/updates', overview({
      phase: 'failed',
      phase_error: 'download failed',
      release_error: 'GitHub unreachable',
      packages_error: 'apt failed',
      run: {
        id: 'run2',
        state: 'failed',
        step: 'debian',
        started_at: 1_790_000_000,
        finished_at: 1_790_000_600,
        zendiloos: null,
        restart_needed: false,
        zfs_ready: false,
        error: 'dpkg was interrupted',
      },
    }))
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

  it('shows a run going on, and a newer version than the page has', async () => {
    let current = '0.1.1'
    on('GET', '/api/v1/system/updates', () =>
      overview({
        current,
        run: {
          id: 'run3',
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
    expect(called('PUT', '/api/v1/system/updates/settings')[0]?.body).toEqual({ channel: 'development', check_daily: true })

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
