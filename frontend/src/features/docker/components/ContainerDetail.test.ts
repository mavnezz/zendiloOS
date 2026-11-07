import type { VueWrapper } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { DockerContainer, DockerContainerDetails } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { WINDOW_SECTION } from '@/shared/lib/window-section'
import { click, labelled, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, reset } from '@/shared/test-support/server'

import { terminalUrl } from '../api'
import { UPDATE_WITHOUT_ASKING } from '../lib/navigation'
import DockerApp from './DockerApp.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

/** Terminals made, for the test to look at. */
const { terminals } = vi.hoisted(() => ({
  terminals: [] as {
    written: unknown[]
    data?: (data: string) => void
    resized?: (size: { cols: number; rows: number }) => void
  }[],
}))

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 24
    written: unknown[] = []
    data?: (data: string) => void
    resized?: (size: { cols: number; rows: number }) => void
    constructor() {
      terminals.push(this)
    }
    loadAddon(): void {}
    open(): void {}
    focus(): void {}
    dispose(): void {}
    write(data: unknown): void {
      this.written.push(data)
    }
    onData(handler: (data: string) => void): void {
      this.data = handler
    }
    onResize(handler: (size: { cols: number; rows: number }) => void): void {
      this.resized = handler
    }
  },
}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit(): void {}
  },
}))

/** A WebSocket that records what is sent; the test plays the server. */
class FakeSocket {
  static readonly OPEN = 1
  static made: FakeSocket[] = []
  readyState = 1
  binaryType = 'blob'
  sent: unknown[] = []
  closed = false
  onopen: (() => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: (() => void) | null = null
  constructor(readonly url: string) {
    FakeSocket.made.push(this)
  }
  send(data: unknown): void {
    this.sent.push(data)
  }
  close(): void {
    this.closed = true
  }
}

const t = (key: string, values: Record<string, unknown> = {}, plural?: number) =>
  plural === undefined ? i18n.global.t(key, values) : i18n.global.t(key, values, plural)

function container(extra: Partial<DockerContainer> = {}): DockerContainer {
  return {
    id: 'id-web',
    name: 'tesla-web',
    image: 'nginx:1.27',
    image_id: 'sha256:nginx',
    state: 'running',
    created_at: 1_790_000_000,
    started_at: 1_790_000_100,
    mounts: [],
    networks: ['tesla_default'],
    ports: [{ container_port: 80, host_port: 8080, protocol: 'tcp', host_ip: null }],
    project: 'tesla',
    service: 'web',
    restart_policy: 'unless-stopped',
    health: null,
    exit_code: null,
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

function details(extra: Partial<DockerContainer> = {}): DockerContainerDetails {
  return {
    container: container(extra),
    command: ['/docker-entrypoint.sh', 'nginx', '-g', 'daemon off;'],
    working_dir: null,
    user: '999:999',
    hostname: 'a1b2c3d4e5f6',
    environment: [
      { name: 'TZ', value: 'Europe/Berlin' },
      { name: 'DB_PASSWORD', value: 'hunter2' },
    ],
    network_mode: 'tesla_default',
    attached_networks: [
      {
        name: 'tesla_default',
        ipv4: '172.19.0.2',
        ipv6: null,
        gateway: '172.19.0.1',
        mac: null,
        aliases: ['web'],
      },
    ],
    detailed_mounts: [
      {
        kind: 'bind',
        source: '/run/zendiloos-project-shares/tesla/3f9a',
        host_path: '/volume1/media/Films',
        destination: '/films',
        read_only: true,
      },
      {
        kind: 'volume',
        source: 'tesla_cache',
        host_path: 'tesla_cache',
        destination: '/cache',
        read_only: false,
      },
      {
        kind: 'bind',
        source: '/etc/localtime',
        host_path: '/etc/localtime',
        destination: '/etc/localtime',
        read_only: true,
      },
    ],
    privileges: {
      privileged: false,
      capabilities_added: ['NET_BIND_SERVICE'],
      capabilities_dropped: ['ALL'],
      devices: [{ host: '/dev/dri', container: '/dev/dri', permissions: 'rwm' }],
      security_options: ['no-new-privileges:true'],
      read_only_root: false,
      pid_mode: '',
      ipc_mode: '',
    },
    limits: {
      cpus: 1.5,
      memory_bytes: 536_870_912,
      memory_reservation_bytes: null,
      processes: null,
    },
    restart_retries: 0,
    healthcheck: {
      test: ['CMD-SHELL', 'pg_isready'],
      interval_seconds: 10,
      timeout_seconds: 5,
      retries: 5,
    },
    health_output: 'accepting connections',
  }
}

const section = ref('containers')

async function open(): Promise<VueWrapper> {
  section.value = 'containers'
  const { wrapper } = await mountApp(DockerApp, {
    global: {
      provide: {
        [WINDOW_SECTION as symbol]: { section, navigate: (next: string) => (section.value = next) },
      },
    },
  })
  return wrapper
}

/** Opens tesla-web's page by its name in the list. */
async function openWeb(): Promise<VueWrapper> {
  const wrapper = await open()
  await click('tesla-web')
  return wrapper
}

/** Switches the tabs of `index` (the page's 0, the overview's parts 1) to `id`. */
async function tab(wrapper: VueWrapper, index: number, id: string): Promise<void> {
  wrapper.findAllComponents({ name: 'PageTabs' })[index]!.vm.$emit('update:modelValue', id)
  await settle()
}

beforeEach(() => {
  reset()
  terminals.length = 0
  FakeSocket.made = []
  vi.stubGlobal('WebSocket', FakeSocket)
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe(): void {}
      disconnect(): void {}
    },
  )
  try {
    window.localStorage.removeItem(UPDATE_WITHOUT_ASKING)
  } catch {
    // Without storage there is nothing to forget.
  }
  on('GET', '/api/v1/docker/containers', [container()])
  on('GET', '/api/v1/docker/containers/tesla-web', details())
  on('GET', '/api/v1/docker/usage', [])
  on('GET', '/api/v1/docker/updates', { checked_at: 1_790_000_000, images: [] })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('a container’s page', () => {
  it('opens by its name and shows what the container is, part by part', async () => {
    const wrapper = await openWeb()

    expect(wrapper.findComponent({ name: 'ContainerDetail' }).props('name')).toBe('tesla-web')
    expect(page()).toContain('nginx:1.27')
    expect(page()).toContain('2.5 %')
    // Folders: a share's folder, a Docker volume, a folder of the system.
    expect(page()).toContain('media/Films')
    expect(page()).toContain(t('docker.details.sharedFolder', { volume: 1 }))
    expect(page()).toContain(t('docker.details.dockerVolume'))
    expect(page()).toContain(t('docker.details.systemPath'))
    expect(page()).not.toContain('zendiloos-project-shares')

    await tab(wrapper, 1, 'networks')
    expect(page()).toContain('172.19.0.2')
    await tab(wrapper, 1, 'ports')
    expect(page()).toContain('8080')
    await tab(wrapper, 1, 'environment')
    expect(page()).toContain('DB_PASSWORD')
    expect(page()).not.toContain('hunter2')
    labelled(t('docker.details.reveal', { name: 'DB_PASSWORD' })).click()
    await settle()
    expect(page()).toContain('hunter2')
    await tab(wrapper, 1, 'privileges')
    expect(page()).toContain('NET_BIND_SERVICE')
    expect(page()).toContain('/dev/dri')
    await tab(wrapper, 1, 'other')
    expect(page()).toContain('pg_isready')
    expect(page()).toContain('accepting connections')
    expect(page()).toContain('999:999')

    labelled(t('docker.back')).click()
    await settle()
    expect(wrapper.findComponent({ name: 'ContainersSection' }).exists()).toBe(true)
  })

  it('lists its processes and follows what it writes', async () => {
    on('GET', '/api/v1/docker/containers/tesla-web/processes', [
      {
        pid: 4242,
        parent: 1,
        user: 'root',
        cpu_percent: 0.5,
        memory_percent: 1,
        resident_bytes: 52_428_800,
        elapsed: '01:02:03',
        command: 'nginx: master process',
      },
    ])
    on('GET', '/api/v1/docker/containers/tesla-web/logs', {
      text: '2026-11-07 ready\n2026-11-07 GET /\n',
    })
    const wrapper = await openWeb()

    await tab(wrapper, 0, 'processes')
    expect(page()).toContain('nginx: master process')
    expect(page()).toContain('01:02:03')

    await tab(wrapper, 0, 'log')
    expect(page()).toContain('GET /')
    await type('input[placeholder]', 'ready')
    expect(page()).not.toContain('GET /')
    const created = vi.fn(() => 'blob:log')
    vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: created, revokeObjectURL: vi.fn() }))
    await click(new RegExp(t('docker.logs.download')))
    expect(created).toHaveBeenCalled()
    expect(
      called('GET', '/api/v1/docker/containers/tesla-web/logs').some(
        (call) => call.query.lines === '500',
      ),
    ).toBe(true)
  })

  it('says a stopped container has no processes, and starts it', async () => {
    on('GET', '/api/v1/docker/containers/tesla-web', details({ state: 'exited', exit_code: 137 }))
    on('POST', '/api/v1/docker/containers/tesla-web/start', null)
    const wrapper = await openWeb()
    expect(page()).toContain('137')

    await tab(wrapper, 0, 'processes')
    expect(page()).toContain(t('docker.details.notRunning'))
    expect(called('GET', '/api/v1/docker/containers/tesla-web/processes')).toHaveLength(0)
    await click(new RegExp(t('docker.actions.start')))
    expect(called('POST', '/api/v1/docker/containers/tesla-web/start')).toHaveLength(1)
  })

  it('stops and restarts a running one', async () => {
    on('POST', '/api/v1/docker/containers/tesla-web/stop', null)
    on('POST', '/api/v1/docker/containers/tesla-web/restart', null)
    await openWeb()
    await click(t('docker.actions.stop'))
    await click(new RegExp(t('docker.actions.restart')))
    expect(called('POST', '/api/v1/docker/containers/tesla-web/stop')).toHaveLength(1)
    expect(called('POST', '/api/v1/docker/containers/tesla-web/restart')).toHaveLength(1)
  })

  it('opens terminals in it, passes what is typed both ways, and ends them', async () => {
    const wrapper = await openWeb()
    await tab(wrapper, 0, 'terminal')
    expect(page()).toContain(t('docker.terminal.none'))

    await click(new RegExp(t('docker.terminal.add')))
    await click('/bin/bash')
    await submit('#terminal-command')
    expect(FakeSocket.made).toHaveLength(1)
    const socket = FakeSocket.made[0]!
    expect(socket.url).toContain(
      '/api/v1/docker/containers/tesla-web/terminal?command=%2Fbin%2Fbash',
    )
    expect(socket.binaryType).toBe('arraybuffer')

    socket.onopen?.()
    socket.onmessage?.({ data: new TextEncoder().encode('root@web:/# ').buffer })
    const terminal = terminals[0]!
    expect(terminal.written).toHaveLength(1)
    terminal.data?.('ls\r')
    expect(new TextDecoder().decode(socket.sent[0] as Uint8Array)).toBe('ls\r')
    terminal.resized?.({ cols: 120, rows: 40 })
    expect(JSON.parse(socket.sent[1] as string)).toEqual({ type: 'resize', columns: 120, rows: 40 })
    socket.onmessage?.({ data: JSON.stringify({ type: 'ended', reason: 'idle' }) })
    expect(String(terminal.written.at(-1))).toContain(t('docker.terminal.ended.idle'))

    // A second one: a refusal is shown and stays.
    await click(new RegExp(t('docker.terminal.add')))
    await submit('#terminal-command')
    const second = FakeSocket.made[1]!
    second.onmessage?.({ data: JSON.stringify({ type: 'error', message: 'not running' }) })
    second.onclose?.()
    await settle()
    expect(String(terminals[1]!.written.at(-1))).toContain('not running')

    labelled(t('docker.terminal.end', { command: '/bin/bash' })).click()
    await settle()
    expect(socket.closed).toBe(true)
  })

  it('refuses a command that is an option', async () => {
    const wrapper = await openWeb()
    await tab(wrapper, 0, 'terminal')
    await click(new RegExp(t('docker.terminal.add')))
    await type('#terminal-command-input', '--privileged sh')
    await submit('#terminal-command')
    expect(FakeSocket.made).toHaveLength(0)
  })
})

describe('a container asked for by the window', () => {
  it('opens as container:<name>, and Back leads to the project it came from', async () => {
    const project = {
      name: 'tesla',
      kind: 'own',
      running: 1,
      containers: [container()],
      files: ['/volume1/docker/tesla/compose.yaml'],
      folder: 'docker/tesla',
      share_id: 1,
      created_at: 1_790_000_000,
      created_by: 'alice',
      app: null,
    }
    on('GET', '/api/v1/docker/projects', [project])
    on('GET', '/api/v1/docker/projects/tesla', project)
    const wrapper = await open()
    section.value = 'projects'
    await settle()
    ;(document.querySelector('li') as HTMLElement).click()
    await settle()
    expect(wrapper.findComponent({ name: 'ProjectDetail' }).exists()).toBe(true)

    section.value = 'container:tesla-web'
    await settle()
    expect(section.value).toBe('containers')
    expect(wrapper.findComponent({ name: 'ContainerDetail' }).props('name')).toBe('tesla-web')
    labelled(t('docker.back')).click()
    await settle()
    expect(section.value).toBe('projects')
    expect(wrapper.findComponent({ name: 'ProjectDetail' }).props('name')).toBe('tesla')
  })

  it('opens from a project’s list of containers by the name', async () => {
    const project = {
      name: 'tesla',
      kind: 'own',
      running: 1,
      containers: [container()],
      files: [],
      folder: 'docker/tesla',
      share_id: 1,
      created_at: 1_790_000_000,
      created_by: 'alice',
      app: null,
    }
    on('GET', '/api/v1/docker/projects', [project])
    on('GET', '/api/v1/docker/projects/tesla', project)
    const wrapper = await open()
    section.value = 'projects'
    await settle()
    ;(document.querySelector('li') as HTMLElement).click()
    await settle()
    await click('tesla-web')
    expect(wrapper.findComponent({ name: 'ContainerDetail' }).props('name')).toBe('tesla-web')
    labelled(t('docker.back')).click()
    await settle()
    expect(wrapper.findComponent({ name: 'ProjectDetail' }).props('name')).toBe('tesla')
  })
})

describe('updating containers', () => {
  beforeEach(() => {
    on('GET', '/api/v1/docker/updates', {
      checked_at: 1_790_000_000,
      images: [
        { reference: 'nginx:1.27', state: 'available', containers: ['tesla-web'], reason: null },
      ],
    })
    on('GET', '/api/v1/docker/containers/id-web/update', { project: 'tesla', image: null })
    on('POST', '/api/v1/docker/containers/id-web/update', {
      id: 7,
      kind: 'containers.update',
      target: 'tesla-web',
      state: 'running',
    })
    on('POST', '/api/v1/docker/updates/apply', {
      job: { id: 8, kind: 'containers.update', target: '1 projects', state: 'running' },
    })
  })

  it('asks before updating one, then not again when told so', async () => {
    await open()
    expect(page()).toContain(t('docker.update.banner', { n: 1 }, 1))

    await click(t('docker.updatable'))
    expect(called('GET', '/api/v1/docker/containers/id-web/update')).toHaveLength(1)
    expect(page()).toContain(
      t('docker.update.byProject', { project: 'tesla', image: 'nginx:1.27' }),
    )
    const remember = document.querySelector<HTMLInputElement>('input[type=checkbox]')!
    remember.click()
    await settle()
    await click(t('docker.update.confirm'))
    expect(called('POST', '/api/v1/docker/containers/id-web/update')).toHaveLength(1)
    expect(window.localStorage.getItem(UPDATE_WITHOUT_ASKING)).toBe('true')
    expect(page()).toContain(t('docker.update.running'))
    // What Compose does shows as it runs.
    expect(page()).toContain(t('docker.deploy.title'))
  })

  it('updates all at once, straight away once told not to ask', async () => {
    await open()
    await click(new RegExp(t('docker.update.allAction')))
    expect(page()).toContain(t('docker.update.all'))
    await click(t('docker.update.confirm'))
    expect(called('POST', '/api/v1/docker/updates/apply')).toHaveLength(1)

    window.localStorage.setItem(UPDATE_WITHOUT_ASKING, 'true')
    await click(t('docker.updatable'))
    expect(called('GET', '/api/v1/docker/containers/id-web/update')).toHaveLength(0)
    expect(called('POST', '/api/v1/docker/containers/id-web/update')).toHaveLength(1)
  })

  it('says how a container outside a project is updated', async () => {
    on('GET', '/api/v1/docker/containers/id-web/update', { project: null, image: 'nginx:1.27' })
    await open()
    await click(t('docker.updatable'))
    expect(page()).toContain(t('docker.update.pullOnly', { image: 'nginx:1.27' }))
  })
})

describe('where a terminal is reached', () => {
  it('on the page’s own host, encrypted when the page is', () => {
    expect(
      terminalUrl('id-web', '/bin/sh -l', 100, 30, { protocol: 'https:', host: 'nas:8443' }),
    ).toBe(
      'wss://nas:8443/api/v1/docker/containers/id-web/terminal?command=%2Fbin%2Fsh+-l&columns=100&rows=30',
    )
    expect(terminalUrl('a b', 'sh', 80, 24, { protocol: 'http:', host: 'localhost' })).toContain(
      'ws://localhost/api/v1/docker/containers/a%20b/terminal',
    )
  })
})
