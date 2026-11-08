import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import { mountApp, settle } from '@/desktop/testing/mount'
import type { DockerContainer, DockerProject } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { on } from '@/shared/test-support/server'

import { findApp } from './apps'
import AppWindow from './AppWindow.vue'
import { useWindows } from './windows'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const web: DockerContainer = {
  id: 'id-tesla-web',
  name: 'tesla-web',
  image: 'nginx:1.27',
  image_id: 'sha256:nginx',
  state: 'running',
  created_at: 1_790_000_000,
  started_at: 1_790_000_100,
  mounts: [],
  networks: [],
  ports: [],
  project: 'tesla',
  service: 'web',
  restart_policy: 'unless-stopped',
  health: null,
  usage: null,
}
const tesla: DockerProject = {
  name: 'tesla',
  kind: 'own',
  running: 1,
  containers: [web],
  files: ['/volume1/docker/tesla/compose.yaml'],
  folder: 'docker/tesla',
  share_id: 1,
  created_at: 1_790_000_000,
  created_by: 'alice',
  app: null,
}

/** The Docker app in a real window of the desktop, as it is opened. */
async function dockerWindow(section: string) {
  const app = findApp('docker')!
  const Host = defineComponent({
    setup() {
      const windows = useWindows()
      windows.resized({ width: 1600, height: 1000 })
      windows.open(app, section)
      return () => {
        const window = windows.state.windows.find((entry) => entry.appId === app.id)
        return window ? h(AppWindow, { app, window, layer: 0, active: true, compact: false }) : null
      }
    },
  })
  const { wrapper } = await mountApp(Host)
  // The app's page is loaded when it is first opened.
  for (let round = 0; round < 5; round += 1) {
    await vi.dynamicImportSettled()
    await settle()
  }
  const windows = useWindows()
  return {
    wrapper,
    section: () => windows.state.windows.find((entry) => entry.appId === app.id)!.section,
  }
}

describe('the Docker app in a window', () => {
  it('opens a container from its project’s page by its row', async () => {
    on('GET', '/api/v1/docker/projects', [tesla])
    on('GET', '/api/v1/docker/projects/tesla', tesla)
    on('GET', '/api/v1/docker/containers', [web])
    on('GET', '/api/v1/docker/containers/tesla-web', {
      container: web,
      command: ['nginx'],
      working_dir: '/',
      user: null,
      hostname: 'web',
      environment: [],
      network_mode: 'tesla_default',
      attached_networks: [],
      detailed_mounts: [],
      privileges: {
        privileged: false,
        capabilities_added: [],
        capabilities_dropped: [],
        devices: [],
        security_options: [],
        read_only_root: false,
        pid_mode: '',
        ipc_mode: 'private',
      },
      limits: { cpus: null, memory_bytes: null, memory_reservation_bytes: null, processes: null },
      restart_retries: 0,
      healthcheck: null,
      health_output: null,
    })
    const { wrapper, section } = await dockerWindow('project:tesla')
    await settle()
    // One way only: the name is no link of its own inside the row.
    expect(
      [...document.querySelectorAll('button')].some((b) => b.textContent?.trim() === 'tesla-web'),
    ).toBe(false)
    const row = document.querySelector<HTMLElement>('[role=button][aria-label*="tesla-web"]')
    expect(row, document.body.textContent ?? '').not.toBeNull()
    row!.click()
    await settle()
    await settle()
    expect(section()).toBe('containers')
    expect(wrapper.findComponent({ name: 'ContainerDetail' }).exists()).toBe(true)

    // Back leads to the project it came from.
    document.querySelector<HTMLElement>(`[aria-label="${i18n.global.t('docker.back')}"]`)!.click()
    await settle()
    await settle()
    expect(section()).toBe('projects')
    expect(wrapper.findComponent({ name: 'ProjectDetail' }).props('name')).toBe('tesla')
  })
})
