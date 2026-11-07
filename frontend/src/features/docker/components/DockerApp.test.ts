import type { VueWrapper } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import {
  button,
  click,
  mountApp,
  page,
  press,
  settle,
  submit,
  type,
} from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { DockerContainer, DockerProject } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'

import DockerApp from './DockerApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

function container(name: string, extra: Partial<DockerContainer> = {}): DockerContainer {
  return {
    id: `id-${name}`,
    name,
    image: 'nginx:1.27',
    image_id: 'sha256:nginx',
    state: 'running',
    created_at: 1_790_000_000,
    started_at: 1_790_000_100,
    mounts: [
      { kind: 'bind', source: '/volume1/web', destination: '/usr/share/nginx', read_only: true },
    ],
    networks: ['tesla_default'],
    ports: [{ container_port: 80, host_port: 8080, protocol: 'tcp', host_ip: null }],
    project: 'tesla',
    service: 'web',
    restart_policy: 'unless-stopped',
    health: 'healthy',
    usage: {
      cpu_percent: 2.5,
      memory_bytes: 50_000_000,
      memory_limit_bytes: 1_000_000_000,
      received_bytes: 10,
      sent_bytes: 20,
    },
    ...extra,
  }
}

function project(name: string, extra: Partial<DockerProject> = {}): DockerProject {
  return {
    name,
    kind: 'own',
    running: 1,
    containers: [container(`${name}-web`, { project: name })],
    files: [`/volume1/docker/${name}/compose.yaml`],
    folder: `docker/${name}`,
    share_id: 1,
    created_at: 1_790_000_000,
    created_by: 'alice',
    app: null,
    ...extra,
  }
}

const section = ref('overview')

async function open(name: string): Promise<VueWrapper> {
  section.value = name
  const { wrapper } = await mountApp(DockerApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: { section, navigate: (next: string) => (section.value = next) },
      },
    },
  })
  return wrapper
}

/** What an action menu or a switch would emit, by its position. */
async function choose(wrapper: VueWrapper, menu: number, action: string): Promise<void> {
  wrapper.findAllComponents({ name: 'ActionMenu' })[menu]!.vm.$emit('select', action)
  await settle()
}

async function confirm(wrapper: VueWrapper): Promise<void> {
  const dialogs = wrapper.findAllComponents({ name: 'ConfirmDialog' })
  const shown = dialogs.find((dialog) => dialog.props('open'))
  if (!shown) throw new Error('no open confirmation')
  shown.vm.$emit('confirm')
  await settle()
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/docker/overview', {
    projects: 2,
    projects_running: 1,
    containers: 3,
    containers_running: 2,
    images: 4,
    data_bytes: 5_000_000_000,
    cpu_share: 12.3,
    memory_bytes: 400_000_000,
    memory_total_bytes: 4_000_000_000,
  })
  on('GET', '/api/v1/docker/usage', [
    {
      at: 1_790_000_000,
      points: [
        {
          container: 'tesla-web',
          cpu_percent: 2,
          memory_bytes: 1e6,
          received_per_second: 1,
          sent_per_second: 2,
        },
      ],
    },
    {
      at: 1_790_000_020,
      points: [
        {
          container: 'tesla-web',
          cpu_percent: 4,
          memory_bytes: 2e6,
          received_per_second: 3,
          sent_per_second: 4,
        },
      ],
    },
  ])
  on('GET', '/api/v1/docker/updates', {
    checked_at: 1_790_000_000,
    images: [
      { reference: 'nginx:1.27', state: 'available', containers: ['tesla-web'], reason: null },
      { reference: 'local/thing', state: 'unknown', containers: ['other'], reason: 'no registry' },
    ],
  })
})

describe('the Docker app', () => {
  it('shows the engine at a glance', async () => {
    await open('overview')
    expect(page()).toContain(t('docker.overview.up'))
    expect(page()).toContain('12 %')
    expect(page()).toContain('10 %')
    expect(called('GET', '/api/v1/docker/usage').length).toBeGreaterThan(0)
  })

  it('says when the engine does not answer', async () => {
    on('GET', '/api/v1/docker/overview', problem(502, 'host_failed', 'no socket'))
    await open('overview')
    expect(page()).toContain(t('docker.overview.down'))
  })

  it('lists projects, runs them and opens one', async () => {
    on('GET', '/api/v1/docker/projects', [
      project('tesla'),
      project('kuma', { kind: 'app', app: 'kuma', running: 0 }),
      project('stack', { kind: 'foreign', running: 0, containers: [] }),
    ])
    on('POST', '/api/v1/docker/projects/tesla/stop', null)
    on('POST', '/api/v1/docker/projects/tesla/redeploy', {
      id: 3,
      kind: 'containers.project_redeploy',
      state: 'running',
    })
    on('DELETE', '/api/v1/docker/projects/tesla', {
      id: 4,
      kind: 'containers.project_delete',
      state: 'running',
    })
    const wrapper = await open('projects')

    expect(page()).toContain('tesla')
    expect(page()).toContain(t('docker.updatable'))
    await type('input', 'kum')
    expect(page()).not.toContain('tesla')
    await type('input', '')

    const stop = [...document.querySelectorAll('li')][0]!.querySelectorAll(
      'button',
    )[1] as HTMLButtonElement
    stop.click()
    await settle()
    expect(called('POST', '/api/v1/docker/projects/tesla/stop')).toHaveLength(1)
    await choose(wrapper, 0, 'redeploy')
    expect(page()).toContain(t('docker.redeploy.title', { name: 'tesla' }))
    await click(new RegExp(`^\\s*${t('docker.actions.redeploy')}\\s*$`))
    expect(called('POST', '/api/v1/docker/projects/tesla/redeploy')[0]!.body).toEqual({
      pull: false,
    })
    expect(page()).toContain(t('docker.deploy.title'))
    await click(new RegExp(t('docker.deploy.hide')))
    await choose(wrapper, 0, 'delete')
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/projects/tesla')).toHaveLength(1)

    on('GET', '/api/v1/docker/projects/tesla', project('tesla'))
    on('GET', '/api/v1/docker/projects/tesla/compose', {
      editable: true,
      files: [{ path: '/volume1/docker/tesla/compose.yaml', content: 'services: {}\n' }],
    })
    on('GET', '/api/v1/docker/projects/tesla/logs', { text: 'web-1  | ready\n' })
    ;(document.querySelector('li') as HTMLElement).click()
    await settle()
    expect(page()).toContain(t('docker.kinds.own'))
    expect(page()).toContain('tesla-web')

    const tabs = wrapper.findComponent({ name: 'PageTabs' })
    tabs.vm.$emit('update:modelValue', 'usage')
    await settle()
    tabs.vm.$emit('update:modelValue', 'logs')
    await settle()
    expect(page()).toContain('ready')
    tabs.vm.$emit('update:modelValue', 'compose')
    await settle()
    expect(wrapper.findComponent({ name: 'CodeEditor' }).props('modelValue')).toBe('services: {}\n')
    await click(new RegExp(t('docker.projects.change')))
    expect(wrapper.findComponent({ name: 'ProjectEditor' }).exists()).toBe(true)
  })

  it('acts on a project from its own page', async () => {
    on('GET', '/api/v1/docker/projects', [project('tesla')])
    on('GET', '/api/v1/docker/projects/tesla', project('tesla', { running: 0 }))
    on('POST', '/api/v1/docker/projects/tesla/start', null)
    on('POST', '/api/v1/docker/projects/tesla/restart', null)
    on('DELETE', '/api/v1/docker/projects/tesla', {
      id: 4,
      kind: 'containers.project_delete',
      state: 'running',
    })
    const wrapper = await open('projects')
    ;(document.querySelector('li') as HTMLElement).click()
    await settle()

    await click(new RegExp(t('docker.actions.start')))
    expect(called('POST', '/api/v1/docker/projects/tesla/start')).toHaveLength(1)
    await click(new RegExp(t('docker.actions.restart')))
    expect(called('POST', '/api/v1/docker/projects/tesla/restart')).toHaveLength(1)
    await choose(wrapper, 0, 'delete')
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/projects/tesla')).toHaveLength(1)
    expect(wrapper.findComponent({ name: 'ProjectsSection' }).exists()).toBe(true)
  })

  it('opens the project another app asks for', async () => {
    on('GET', '/api/v1/docker/projects', [project('tesla')])
    on('GET', '/api/v1/docker/projects/tesla', project('tesla'))
    const wrapper = await open('project:tesla')
    await settle()

    expect(section.value).toBe('projects')
    expect(wrapper.findComponent({ name: 'ProjectDetail' }).props('name')).toBe('tesla')
  })

  it('tells an app’s project from one made here', async () => {
    on('GET', '/api/v1/docker/projects', [
      project('kuma', { kind: 'app', app: 'kuma', containers: [] }),
    ])
    on(
      'GET',
      '/api/v1/docker/projects/kuma',
      project('kuma', { kind: 'app', app: 'kuma', containers: [] }),
    )
    const wrapper = await open('projects')
    ;(document.querySelector('li') as HTMLElement).click()
    await settle()
    expect(page()).toContain(t('docker.projects.appHint'))
    wrapper.findComponent({ name: 'PageTabs' }).vm.$emit('update:modelValue', 'compose')
    await settle()
    expect(page()).toContain(t('docker.projects.appCompose'))
    await press(t('docker.back'))
  })

  it('starts making a project', async () => {
    on('GET', '/api/v1/docker/projects', [])
    on('GET', '/api/v1/docker/places', [{ share: 'docker', share_id: 1 }])
    const wrapper = await open('projects')
    expect(page()).toContain(t('docker.projects.none'))
    await click(new RegExp(t('docker.projects.create')))
    expect(wrapper.findComponent({ name: 'ProjectEditor' }).exists()).toBe(true)
  })

  it('lists containers, reads their log and runs them', async () => {
    on('GET', '/api/v1/docker/containers', [
      container('tesla-web'),
      container('old', { state: 'exited', exit_code: 1, usage: null, project: null, health: null }),
    ])
    on('GET', '/api/v1/docker/containers/id-tesla-web/logs', { text: 'listening on :80\n' })
    on('POST', '/api/v1/docker/containers/id-tesla-web/restart', null)
    on('POST', '/api/v1/docker/containers/id-old/start', null)
    on('DELETE', '/api/v1/docker/containers/id-old', null)
    const wrapper = await open('containers')

    expect(page()).toContain('tesla-web')
    expect(page()).toContain('8080')
    await choose(wrapper, 0, 'logs')
    expect(page()).toContain('listening on :80')
    await choose(wrapper, 0, 'restart')
    expect(called('POST', '/api/v1/docker/containers/id-tesla-web/restart')).toHaveLength(1)
    wrapper.findAllComponents({ name: 'Switch' })[1]!.vm.$emit('update:modelValue', true)
    await settle()
    expect(called('POST', '/api/v1/docker/containers/id-old/start')).toHaveLength(1)
    await choose(wrapper, 1, 'remove')
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/containers/id-old')).toHaveLength(1)
    await type('input', 'nothing like it')
    expect(page()).toContain(t('docker.containers.none'))
  })

  it('pulls, removes and cleans up images', async () => {
    on('GET', '/api/v1/docker/images', [
      {
        id: 'sha256:aaaaaaaaaaaa1111',
        references: ['nginx:1.27'],
        digests: [],
        size_bytes: 70_000_000,
        created_at: 1_790_000_000,
        used_by: ['tesla-web'],
      },
      {
        id: 'sha256:bbbbbbbbbbbb2222',
        references: [],
        digests: [],
        size_bytes: 1_000,
        created_at: null,
        used_by: [],
      },
    ])
    on('POST', '/api/v1/docker/images/pull', {
      id: 5,
      kind: 'containers.image_pull',
      state: 'running',
    })
    on('DELETE', '/api/v1/docker/images/sha256:bbbbbbbbbbbb2222', null)
    on('POST', '/api/v1/docker/images/prune', { text: 'Total reclaimed space: 1kB\n' })
    const wrapper = await open('images')

    expect(page()).toContain('nginx:1.27')
    expect(page()).toContain(t('docker.images.unused'))
    await click(t('docker.images.pull'))
    await type(`input[aria-label="${t('docker.images.reference')}"]`, 'postgres:17')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/docker/images/pull')[0]?.body).toEqual({
      reference: 'postgres:17',
    })

    const remove = [
      ...document.querySelectorAll<HTMLButtonElement>(
        `button[aria-label="${t('docker.actions.delete')}"]`,
      ),
    ]
    expect(remove.filter((b) => b.disabled)).toHaveLength(1)
    remove.find((b) => !b.disabled)!.click()
    await settle()
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/images/sha256:bbbbbbbbbbbb2222')).toHaveLength(1)

    await click(new RegExp(t('docker.images.prune')))
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/images/prune')).toHaveLength(1)
    await type(`input[aria-label="${t('docker.search')}"]`, 'postgres')
    expect(page()).not.toContain('nginx:1.27')
  })

  it('makes and removes networks', async () => {
    on('GET', '/api/v1/docker/networks', [
      {
        id: 'n1',
        name: 'bridge',
        driver: 'bridge',
        built_in: true,
        internal: false,
        subnets: ['172.17.0.0/16'],
        gateways: ['172.17.0.1'],
        containers: [],
        project: null,
      },
      {
        id: 'n2',
        name: 'proxy',
        driver: 'bridge',
        built_in: false,
        internal: true,
        subnets: [],
        gateways: [],
        containers: [],
        project: 'tesla',
      },
      {
        id: 'n3',
        name: 'used',
        driver: 'bridge',
        built_in: false,
        internal: false,
        subnets: [],
        gateways: [],
        containers: ['tesla-web'],
        project: null,
      },
    ])
    on('POST', '/api/v1/docker/networks', { id: 'n3', name: 'backend' })
    on('DELETE', '/api/v1/docker/networks/proxy', null)
    const wrapper = await open('networks')

    expect(page()).toContain('172.17.0.0/16')
    await click(new RegExp(t('docker.networks.add')))
    await type('#network-name', 'backend')
    await type('#network-subnet', '10.10.0.0/24')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/docker/networks')[0]?.body).toEqual({
      name: 'backend',
      subnet: '10.10.0.0/24',
      gateway: null,
    })

    const remove = [
      ...document.querySelectorAll<HTMLButtonElement>(
        `button[aria-label="${t('docker.actions.delete')}"]`,
      ),
    ]
    expect(remove.map((b) => b.disabled)).toEqual([true, false, true])
    remove[1]!.click()
    await settle()
    await confirm(wrapper)
    expect(called('DELETE', '/api/v1/docker/networks/proxy')).toHaveLength(1)
  })

  it('shows what was done with Docker', async () => {
    on('GET', '/api/v1/logs/audit', {
      entries: [
        {
          id: 2,
          at: 1_790_000_000,
          action: 'containers.project_create',
          outcome: 'success',
          actor: 'alice',
          target: 'tesla',
        },
        {
          id: 1,
          at: 1_789_000_000,
          action: 'containers.something_new',
          outcome: 'failure',
          actor: 'bob',
          target: null,
          reason: 'agent_failed',
        },
      ],
      older: null,
    })
    await open('activity')
    expect(page()).toContain('tesla')
    expect(page()).toContain('containers.something_new')
    await click(new RegExp(t('docker.activity.refresh')))
    expect(called('GET', '/api/v1/logs/audit').length).toBeGreaterThan(1)
  })

  it('looks for newer images on a schedule and moves Docker’s data', async () => {
    on('GET', '/api/v1/docker/updates/settings', { check: true, schedule: null })
    on('PUT', '/api/v1/docker/updates/settings', (call: { body: unknown }) => call.body)
    on('POST', '/api/v1/docker/updates/check', { checked_at: 1_790_000_500, images: [] })
    on('POST', '/api/v1/docker/updates/apply', {
      id: 6,
      kind: 'containers.update',
      state: 'running',
    })
    on('GET', '/api/v1/docker/data', {
      current: { pool: 'tank', volume: 1, available_bytes: 1e12 },
      data_bytes: 5e9,
      targets: [
        { pool: 'fast', volume: 2, available_bytes: 1e12 },
        { pool: 'small', volume: 3, available_bytes: 1e6 },
      ],
    })
    on('POST', '/api/v1/docker/data/move', {
      id: 7,
      kind: 'containers.data_move',
      state: 'running',
    })
    const wrapper = await open('management')

    expect(page()).toContain('nginx:1.27')
    expect(page()).toContain('local/thing')
    const scheduled = document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')[1]!
    scheduled.click()
    await settle()
    await click(t('docker.management.days.3'))
    await type('input[type="time"]', '04:30')
    wrapper.findComponent({ name: 'PageFooter' }).vm.$emit('apply')
    await settle()
    expect(called('PUT', '/api/v1/docker/updates/settings')[0]?.body).toEqual({
      check: true,
      schedule: { days: [0, 3], time: '04:30' },
    })

    await click(new RegExp(t('docker.management.updateNow')))
    expect(called('POST', '/api/v1/docker/updates/apply')).toHaveLength(1)
    await click(new RegExp(t('docker.management.checkNow')))
    expect(called('POST', '/api/v1/docker/updates/check')).toHaveLength(1)
    expect(page()).not.toContain('local/thing')

    await click(new RegExp(t('docker.management.migrate')))
    expect(button(/small/).disabled).toBe(true)
    await click(/fast/)
    await click(t('docker.management.start'))
    await confirm(wrapper)
    expect(called('POST', '/api/v1/docker/data/move')[0]?.body).toEqual({ pool: 'fast' })
  })
})
