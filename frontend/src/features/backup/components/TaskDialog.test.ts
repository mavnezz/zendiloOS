import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import { i18n } from '@/shared/i18n'
import { button, click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import TaskDialog from './TaskDialog.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)
const picks = ['photos/albums', 'backups']
vi.mock('@/shared/ui/folder-picker', () => ({
  useShareList: () => ({
    data: ref([
      { id: 1, name: 'photos', writable: true },
      { id: 2, name: 'backups', writable: true },
    ]),
  }),
  // Offers a button per folder to pick.
  FolderPicker: defineComponent({
    props: { open: Boolean, value: String, writable: Boolean },
    emits: ['pick', 'update:open'],
    setup:
      (props, { emit }) =>
      () =>
        props.open
          ? h(
              'div',
              picks.map((path) =>
                h('button', { type: 'button', onClick: () => emit('pick', path) }, `pick:${path}`),
              ),
            )
          : null,
  }),
}))

const t = (key: string, values: Record<string, unknown> | number = {}) =>
  typeof values === 'number' ? i18n.global.t(key, values) : i18n.global.t(key, values)

const KEY = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl'
const PLUGIN = { installed: true, ssh: true, public_key: null }

async function wizard() {
  const open = ref(true)
  const mounted = await mountApp(TaskDialog, {
    props: {
      open: true,
      plugin: PLUGIN,
      taken: ['Existing'],
      'onUpdate:open': (value: boolean) => (open.value = value),
    },
  })
  return { ...mounted, open }
}

function radio(text: string): HTMLInputElement {
  const found = [...document.querySelectorAll<HTMLInputElement>('input[type="radio"]')].find(
    (input) => input.closest('label')?.textContent?.includes(text),
  )
  if (!found) throw new Error(`no choice ${text}`)
  return found
}

async function choose(text: string): Promise<void> {
  const input = radio(text)
  input.click()
  input.dispatchEvent(new Event('change'))
  await settle()
}

function problems(): string[] {
  return [...document.querySelectorAll('[data-problem]')].map((item) =>
    item.getAttribute('data-problem')!,
  )
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/storage/removable', [
    {
      id: '1A2B-3C4D',
      label: 'STICK',
      filesystem: 'fat',
      state: 'mounted',
      device: 'sdf1',
      disk: 'sdf',
      read_only: false,
      size_bytes: 1,
      path: '/run/zendiloos-removable/1A2B-3C4D',
    },
  ])
  on('GET', '/api/v1/backup/shares/2/readers', {
    accounts: ['alice'],
    groups: ['family'],
    nfs: ['192.168.1.0/24'],
  })
  on('POST', '/api/v1/backup/tasks', (call: { body: unknown }) => ({
    id: 5,
    ...(call.body as object),
  }))
})

describe('the new task wizard', () => {
  it('backs up a share to another pool in four steps, warning who reads the target', async () => {
    await wizard()
    expect(page()).toContain(t('backup.later'))
    await click(t('backup.wizard.next'))
    expect(button(t('backup.wizard.next')).disabled).toBe(true)
    expect(problems()).toEqual(['name', 'source', 'target'])
    await type('#backup-name', 'Existing')
    expect(problems()).toContain('nameTaken')
    await type('#backup-name', 'Albums nightly')
    await click(new RegExp(`${t('backup.wizard.choose')}`))
    await click('pick:photos/albums')
    const choosers = [...document.querySelectorAll<HTMLButtonElement>('button')].filter((b) =>
      b.textContent?.includes(t('backup.wizard.choose')),
    )
    choosers.at(-1)!.click()
    await settle()
    await click('pick:backups')
    expect(page()).toContain(t('backup.wizard.targetHint', { folder: 'Albums nightly' }))
    expect(document.querySelector('[data-testid="backup-readers"]')?.textContent).toContain(
      'alice, @family, 192.168.1.0/24',
    )
    expect(problems()).toEqual([])
    await click(t('backup.wizard.next'))

    // Options: versions kept by number and age, a pattern added and one taken away.
    expect(page()).toContain(t('backup.modeHints.versioned'))
    await type('#backup-pattern', '- bad')
    await click(t('backup.wizard.add'))
    expect(page()).toContain(t('backup.wizard.badPattern'))
    await type('#backup-pattern', '/cache')
    document
      .querySelector<HTMLInputElement>('#backup-pattern')!
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    await settle()
    document
      .querySelector<HTMLButtonElement>(
        `button[aria-label="${t('backup.wizard.removePattern', { pattern: 'Thumbs.db' })}"]`,
      )!
      .click()
    await settle()
    await type('#backup-frequency', 'weekly')
    await type('#backup-weekday', '6')
    await type('#backup-time', '22:30')
    await choose(t('backup.wizard.bandwidths.custom'))
    await type(`input[aria-label="${t('backup.wizard.kibPerSecond')}"]`, '2048')
    await click(t('backup.wizard.next'))

    const review = document.querySelector('[data-testid="backup-review"]')?.textContent ?? ''
    expect(review).toContain('photos › albums')
    expect(review).toContain('Sunday at 22:30')
    expect(review).toContain('/cache')
    expect(review).toContain('2048')
    await click(t('backup.wizard.create'))
    const body = called('POST', '/api/v1/backup/tasks')[0]?.body as Record<string, unknown>
    expect(body).toMatchObject({
      name: 'Albums nightly',
      source: { kind: 'share', share_id: 1, path: 'albums' },
      target: { kind: 'share', share_id: 2, path: '' },
      server: null,
      schedule: { frequency: 'weekly', weekday: 6, hour: 22, minute: 30 },
      bwlimit_kib: 2048,
    })
    expect(body.excludes).toContain('/cache')
    expect(body.excludes).not.toContain('Thumbs.db')
  })

  it('pushes over SSH once the NAS’s key is shown and the server’s host key pinned', async () => {
    on('POST', '/api/v1/backup/key', { public_key: 'ssh-ed25519 AAAAPUBLIC zendiloos-backup' })
    on('POST', '/api/v1/backup/servers/host-keys', [
      { line: KEY, fingerprint: 'SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU' },
    ])
    on('POST', '/api/v1/backup/servers/check', [])
    await wizard()
    await choose(t('backup.kinds.push'))
    await click(t('backup.wizard.next'))
    await type('#backup-name', 'Offsite')
    await click(new RegExp(`${t('backup.wizard.choose')}`))
    await click('pick:photos/albums')
    await type('#backup-host', 'nas2.lan')
    await type('#backup-user', 'backup')
    await type('#backup-server-path', '/srv/backups')
    expect(problems()).toEqual(['hostKey'])
    await click(t('backup.server.makeKey'))
    expect(document.querySelector('[data-testid="backup-public-key"]')?.textContent).toContain(
      'AAAAPUBLIC',
    )
    await click(new RegExp(t('backup.server.fetchHostKey')))
    expect(called('POST', '/api/v1/backup/servers/host-keys')[0]?.body).toEqual({
      host: 'nas2.lan',
      port: 22,
    })
    expect(page()).toContain('SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU')
    await click(t('backup.server.confirmKey'))
    expect(document.querySelector('[data-testid="backup-pinned"]')?.textContent).toContain(
      'SHA256:+DiY3',
    )
    await click(new RegExp(t('backup.server.check')))
    expect(page()).toContain(t('backup.server.checkEmpty'))
    expect(problems()).toEqual([])
    await click(t('backup.wizard.next'))
    await choose(t('backup.modes.mirror'))
    await click(t('backup.wizard.next'))
    await click(t('backup.wizard.create'))
    expect(called('POST', '/api/v1/backup/tasks')[0]?.body).toMatchObject({
      target: { kind: 'server', path: '/srv/backups' },
      server: { transport: 'ssh', host: 'nas2.lan', port: 22, user: 'backup', host_key: KEY },
      mode: { kind: 'mirror', keep_changed_days: 30 },
    })
  })

  it('pulls a server through a daemon onto a drive, and says what the API refused', async () => {
    on(
      'POST',
      '/api/v1/backup/servers/check',
      problem(502, 'agent_failed', 'rsync: failed to connect'),
    )
    on('POST', '/api/v1/backup/tasks', problem(409, 'folder_taken', 'Albums'))
    await wizard()
    await choose(t('backup.kinds.pull'))
    await click(t('backup.wizard.next'))
    await type('#backup-name', 'Web server')
    await choose(t('backup.server.transports.daemon'))
    expect(page()).toContain(t('backup.server.daemonWarning'))
    expect(document.querySelector<HTMLInputElement>('#backup-port')?.value).toBe('873')
    await type('#backup-host', 'www.lan')
    await type('#backup-user', 'nas')
    await type('#backup-module', 'www')
    await type('#backup-password', 's3cret')
    await click(new RegExp(t('backup.server.check')))
    expect(page()).toContain('rsync: failed to connect')
    await choose(t('backup.wizard.targetDrive'))
    await type('select', '1A2B-3C4D')
    expect(page()).toContain(t('backup.wizard.driveNoLinks', { filesystem: 'fat' }))
    await type('#backup-drive-path', 'servers')
    await click(t('backup.wizard.next'))
    await choose(t('backup.modes.mirror'))
    const plug = [...document.querySelectorAll('label')].find((label) =>
      label.textContent?.includes(t('backup.wizard.onPlug')),
    )!
    plug.querySelector<HTMLButtonElement>('button')!.click()
    await settle()
    await click(t('backup.wizard.next'))
    await click(t('backup.wizard.create'))
    expect(called('POST', '/api/v1/backup/tasks')[0]?.body).toMatchObject({
      source: { kind: 'server', path: '' },
      target: { kind: 'drive', drive: '1A2B-3C4D', path: 'servers' },
      server: { transport: 'daemon', module: 'www' },
      password: 's3cret',
      on_plug: true,
    })
    expect(page()).toContain(t('errors.folder_taken'))
    await click(t('backup.wizard.back'))
    expect(page()).toContain(t('backup.wizard.filters'))
  })
})
