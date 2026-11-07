import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { DockerContainer, DockerProject } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatRate } from '@/shared/lib/format'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'
import { job } from '@/shared/testing/fixtures'

import ProjectDetail from './ProjectDetail.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function container(name: string, extra: Partial<DockerContainer> = {}): DockerContainer {
  return {
    id: `id-${name}`,
    name,
    image: 'teslamate/teslamate:latest',
    image_id: 'sha256:t',
    state: 'running',
    created_at: 1_790_000_000,
    started_at: 1_790_000_100,
    mounts: [],
    networks: ['tesla_default'],
    ports: [],
    project: 'tesla',
    service: 'web',
    restart_policy: 'unless-stopped',
    health: null,
    usage: {
      cpu_percent: 1,
      memory_bytes: 71_000_000,
      memory_limit_bytes: 31_000_000_000,
      received_bytes: 10,
      sent_bytes: 20,
    },
    ...extra,
  }
}

const TESLA: DockerProject = {
  name: 'tesla',
  kind: 'own',
  running: 2,
  containers: [container('tesla-web-1'), container('tesla-db-1', { image: 'postgres:18-trixie' })],
  files: ['/volume1/docker/tesla/compose.yaml'],
  compose_file: 'docker/tesla/compose.yaml',
  folder: 'tesla',
  share_id: 1,
  created_at: 1_790_000_000,
  created_by: 'alice',
  app: null,
}

const section = ref('projects')

async function open(): Promise<VueWrapper> {
  const { wrapper } = await mountApp(ProjectDetail, {
    props: { name: 'tesla' },
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: { section, navigate: (next: string) => (section.value = next) },
      },
    },
  })
  return wrapper
}

async function tab(wrapper: VueWrapper, id: string): Promise<void> {
  wrapper.findComponent({ name: 'PageTabs' }).vm.$emit('update:modelValue', id)
  await settle()
}

beforeEach(() => {
  reset()
  section.value = 'projects'
  on('GET', '/api/v1/docker/projects/tesla', TESLA)
  on('GET', '/api/v1/docker/usage', [
    {
      at: 1_790_000_000,
      points: ['tesla-web-1', 'tesla-db-1'].map((name, n) => ({
        container: name,
        cpu_percent: 1 + n,
        memory_bytes: 1e6,
        received_per_second: 1600 + n,
        sent_per_second: 2600 + n,
      })),
    },
  ])
  on('GET', '/api/v1/docker/projects/tesla/logs', {
    text: [
      'tesla-db-1   | database system is ready',
      'tesla-web-1  | System started',
      'tesla-web-1  | GET /api 200',
    ].join('\n'),
  })
  on('POST', '/api/v1/docker/projects/tesla/redeploy', job(7, 'containers.project_deploy'))
})

describe('a project’s page', () => {
  it('says where its file is and shows each container with what its network does now', async () => {
    await open()
    expect(page()).toContain('docker/tesla/compose.yaml')
    expect(page()).toContain('alice')
    expect(page()).toContain('postgres')
    expect(page()).toContain('18-trixie')
    expect(page()).toContain(formatRate(2600))
    expect(page()).toContain(formatRate(1601))
  })

  it('opens a container’s own page', async () => {
    const wrapper = await open()
    const row = document.querySelector<HTMLElement>(
      `[aria-label="${t('docker.projects.openContainer', { name: 'tesla-db-1' })}"]`,
    )!
    row.click()
    await settle()
    expect(section.value).toBe('container:tesla-db-1')
    row.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(wrapper.emitted('container')).toEqual([['tesla-db-1'], ['tesla-db-1']])
  })

  it('redeploys with the newest images when asked, showing what Compose writes', async () => {
    const wrapper = await open()
    wrapper.findComponent({ name: 'ActionMenu' }).vm.$emit('select', 'redeploy')
    await settle()
    expect(page()).toContain(t('docker.redeploy.text'))
    const pull = document.querySelector<HTMLInputElement>('input[type=checkbox]')!
    expect(pull.checked).toBe(false)
    pull.click()
    await click(new RegExp(`^\\s*${t('docker.actions.redeploy')}\\s*$`))

    expect(called('POST', '/api/v1/docker/projects/tesla/redeploy')[0]!.body).toEqual({
      pull: true,
    })
    expect(page()).toContain(t('docker.deploy.waiting'))
    const jobs = useJobsStore()
    jobs.update(job(7, 'containers.project_deploy', { output: [' app Pulling', ' app Pulled'] }))
    await settle()
    expect(document.querySelector('[data-testid=deploy-output]')!.textContent).toBe(
      ' app Pulling\n app Pulled',
    )
    jobs.update(
      job(7, 'containers.project_deploy', {
        state: 'failed',
        error_detail: 'Docker Compose: pull access denied',
        output: [' app Pulling', ' app Error'],
      }),
    )
    await settle()
    expect(page()).toContain(t('docker.deploy.failed'))
    expect(page()).toContain('pull access denied')
    await click(new RegExp(`^\\s*${t('docker.deploy.ok')}\\s*$`))
    expect(page()).not.toContain(t('docker.deploy.title'))
  })

  it('shows all its containers’ lines under their names, searchable and to download', async () => {
    const wrapper = await open()
    await tab(wrapper, 'logs')
    const names = [...document.querySelectorAll('[data-testid=log-container]')]
    expect(names.map((name) => name.textContent)).toEqual([
      'tesla-db-1',
      'tesla-web-1',
      'tesla-web-1',
    ])
    expect(names[0]!.className).not.toBe(names[1]!.className)
    expect(called('GET', '/api/v1/docker/projects/tesla/logs')[0]!.query).toEqual({ lines: '500' })

    await type('input[type=search]', 'started')
    expect(page()).toContain('System started')
    expect(page()).not.toContain('GET /api 200')
    expect(page()).toContain(t('docker.projectLog.matches', { shown: 1, all: 3 }))
    await type('input[type=search]', 'nothing like it')
    expect(page()).toContain(t('docker.projectLog.noMatch'))

    const create = vi.fn(() => 'blob:log')
    const revoke = vi.fn()
    Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke })
    const clicked = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
    document
      .querySelector<HTMLButtonElement>(`[aria-label="${t('docker.projectLog.download')}"]`)!
      .click()
    expect(create).toHaveBeenCalled()
    expect(clicked).toHaveBeenCalled()
    expect(revoke).toHaveBeenCalledWith('blob:log')
    clicked.mockRestore()
  })

  it('charts what each container used, four measures', async () => {
    const wrapper = await open()
    await tab(wrapper, 'usage')
    for (const metric of ['cpu', 'memory', 'sent', 'received'])
      expect(
        document.querySelector(`section[aria-label="${t(`docker.usage.${metric}`)}"]`),
      ).not.toBe(null)
  })
})
