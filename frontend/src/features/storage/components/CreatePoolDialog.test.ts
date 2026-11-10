import { beforeEach, describe, expect, it, vi } from 'vitest'

import { button, click, mountApp, page, settle, submit, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'
import type { Disk, PoolPlan } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { formatBytes } from '@/shared/lib/format'

import CreatePoolDialog from './CreatePoolDialog.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> | number = {}) =>
  typeof values === 'number' ? i18n.global.t(key, values) : i18n.global.t(key, values)
const TB = 1e12

function disk(name: string, extra: Partial<Disk> = {}): Disk {
  return {
    name,
    model: `Disk ${name}`,
    serial: `S-${name}`,
    size_bytes: 4 * TB,
    media: 'rotational',
    transport: 'sata',
    usage: 'unused',
    id_link: `/dev/disk/by-id/ata-${name}`,
    partitions: [],
    read_only: false,
    removable: false,
    pool: null,
    array: null,
    wwn: null,
    ...extra,
  }
}

const PLAN: PoolPlan = {
  name: 'tank',
  layout: 'mirror',
  redundancy: 1,
  parity: null,
  capacity_bytes: 2 * TB,
  mount_point: '/volume1',
  volume: 1,
  token: 'plan-1',
  expires_at: 2_000_000_000,
  disks: [],
  warnings: [],
}

function checkbox(name: string): HTMLInputElement {
  return document.querySelector<HTMLInputElement>(`input[type="checkbox"][value="${name}"]`)!
}

function radio(layout: string): HTMLInputElement {
  return document.querySelector<HTMLInputElement>(`input[type="radio"][value="${layout}"]`)!
}

async function choose(...names: string[]): Promise<void> {
  for (const name of names) {
    checkbox(name).click()
    await settle()
  }
}

/** The text of the layout card for `layout`. */
function card(layout: string): string {
  return document.querySelector(`[data-layout="${layout}"]`)?.textContent ?? ''
}

function heading(): string {
  return document.querySelector('[role="dialog"] h3')?.textContent?.replace(/\s+/g, ' ') ?? ''
}

async function mountDialog() {
  return mountApp(CreatePoolDialog, { props: { open: true } })
}

beforeEach(() => {
  reset()
  on('GET', '/api/v1/storage/disks', [
    disk('sda', { usage: 'system' }),
    disk('sdc'),
    disk('sdd', {
      size_bytes: 2 * TB,
      media: 'solid_state',
      transport: 'nvme',
      usage: 'foreign',
      model: null,
    }),
    disk('sde'),
  ])
  on('GET', '/api/v1/storage/enclosure', {
    machine: null,
    source: 'custom',
    chassis: { style: 'plain', panel: null },
    groups: [
      {
        id: 'front',
        name: 'Front',
        rows: 1,
        columns: 4,
        numbering: 'left_to_right',
        orientation: 'horizontal',
        slots: [{ id: 'bay3', label: 'Bay 3', state: 'present', disk: 'sde', overridden: false }],
      },
    ],
    unplaced: [],
  })
})

describe('CreatePoolDialog', () => {
  it('shows the free disks as cards', async () => {
    await mountDialog()
    expect(heading()).toContain(t('storage.create.steps.disks'))
    expect(heading()).toContain(t('storage.create.stepOf', { current: 1, total: 3 }))
    const text = page()
    expect(text).not.toContain('sda')
    expect(text).toContain(formatBytes(4 * TB))
    expect(text).toContain(t('storage.usage.foreign'))
    expect(text).toContain(t('storage.unknownModel'))
    expect(text).toContain('Bay 3')
    expect(text).toContain(
      `${t('storage.create.media.solid_state')} · ${t('storage.transport.nvme')}`,
    )
    expect(text).toContain(
      `${t('storage.create.media.rotational')} · ${t('storage.transport.sata')}`,
    )
  })

  it('says when there are no free disks', async () => {
    on('GET', '/api/v1/storage/disks', [disk('sda', { usage: 'system' })])
    await mountDialog()
    expect(page()).toContain(t('storage.create.noDisks'))
    expect(button(t('storage.create.next')).disabled).toBe(true)
  })

  it('estimates each layout for the chosen disks and switches to one that fits', async () => {
    await mountDialog()
    expect(button(t('storage.create.next')).disabled).toBe(true)
    // Nothing chosen: every layout may be picked, with no figures yet.
    expect(document.querySelectorAll('input[type="radio"]:disabled')).toHaveLength(0)
    expect(card('mirror')).toContain(`${t('storage.create.usable')}–`)

    await choose('sdc')
    expect(radio('single').checked).toBe(true)
    expect(radio('mirror').disabled).toBe(true)
    expect(card('mirror')).toContain(t('storage.create.misfit.tooFew', { count: 2 }))
    expect(card('single')).toContain(t('storage.create.strength.none'))
    expect(card('single')).toContain(t('storage.create.layouts.single.cons'))
    expect(button(t('storage.create.next')).disabled).toBe(false)

    await choose('sdd')
    expect(radio('mirror').checked).toBe(true)
    expect(radio('single').disabled).toBe(true)
    expect(card('single')).toContain(t('storage.create.misfit.exactly', 1))
    expect(card('mirror')).toContain(`≈ ${formatBytes(2 * TB)}`)
    expect(card('mirror')).toContain(t('storage.create.speedRead', { count: 2 }))
    expect(card('mirror')).toContain(t('storage.create.speedWrite', { count: 1 }))
    expect(card('mirror')).toContain(t('storage.create.tolerates', 1))

    await choose('sde')
    radio('raidz1').click()
    await settle()
    expect(card('raidz1')).toContain(`≈ ${formatBytes(4 * TB)}`)
    expect(card('raidz1')).toContain(t('storage.create.speedWrite', { count: 2 }))
    expect(card('raidz1')).toContain(t('storage.create.strength.medium'))
    expect(card('raidz1')).toContain(t('storage.create.layouts.raidz1.pros'))
    expect(card('mirror')).not.toContain(t('storage.create.layouts.mirror.pros'))
    expect(card('mirror')).toContain(t('storage.create.strength.strong'))
    expect(card('raidz2')).toContain(t('storage.create.misfit.tooFew', { count: 4 }))
    // The footer splits the 10 TB: 4 usable, 2 for parity, 4 unused on the larger disks.
    const legend = document.querySelector(
      'ul[aria-labelledby="pool-estimate-heading"]',
    )!.textContent
    expect(legend).toContain(`${t('storage.create.legend.usable')} ${formatBytes(4 * TB)}`)
    expect(legend).toContain(`${t('storage.create.legend.protection')} ${formatBytes(2 * TB)}`)
    expect(legend).toContain(`${t('storage.create.legend.unused')} ${formatBytes(4 * TB)}`)
  })

  it('takes no more disks than a pool may have', async () => {
    on(
      'GET',
      '/api/v1/storage/disks',
      Array.from({ length: 13 }, (_, index) => disk(`sd${String.fromCharCode(98 + index)}`)),
    )
    await mountDialog()
    for (const box of document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')) {
      box.click()
      await settle()
    }
    expect(card('raidz3')).toContain(t('storage.create.misfit.tooMany', { count: 12 }))
    expect(button(t('storage.create.next')).disabled).toBe(true)
    await choose('sdb')
    expect(radio('raidz3').disabled).toBe(false)
    expect(card('raidz3')).toContain(t('storage.create.strength.very_strong'))
    expect(card('raidz3')).toContain(t('storage.create.tolerates', 3))
    expect(button(t('storage.create.next')).disabled).toBe(false)
  })

  it('drops a chosen disk that is no longer free', async () => {
    const { queryClient } = await mountDialog()
    await choose('sdc', 'sde')
    on('GET', '/api/v1/storage/disks', [disk('sdc'), disk('sde', { usage: 'pool_member' })])
    await queryClient.invalidateQueries({ queryKey: ['storage', 'disks'] })
    await settle()
    expect(checkbox('sdc').checked).toBe(true)
    expect(checkbox('sde')).toBeNull()
    expect(radio('single').checked).toBe(true)
  })

  it('goes back to the disks and keeps what was chosen', async () => {
    await mountDialog()
    await choose('sdc', 'sde')
    await click(t('storage.create.next'))
    expect(heading()).toContain(t('storage.create.stepOf', { current: 2, total: 3 }))
    expect(document.activeElement?.id).toBe('pool-name')
    expect(page()).toContain(`${t('storage.create.diskCount', 2)} (sdc, sde)`)
    expect(page()).toContain(t('storage.create.layouts.mirror.name'))
    expect(button(t('storage.create.next')).disabled).toBe(true)
    await type('#pool-name', 'tank')

    await click(t('desktop.back'))
    expect(heading()).toContain(t('storage.create.steps.disks'))
    expect(document.activeElement?.tagName).toBe('H3')
    expect(checkbox('sdc').checked).toBe(true)
    expect(checkbox('sde').checked).toBe(true)
    expect(radio('mirror').checked).toBe(true)

    await click(t('storage.create.next'))
    expect(document.querySelector<HTMLInputElement>('#pool-name')!.value).toBe('tank')
  })

  it('stays at the name when the plan is refused', async () => {
    on('POST', '/api/v1/storage/pools/plan', problem(422, 'refused', 'tank exists'))
    await mountDialog()
    await choose('sdc', 'sde')
    await click(t('storage.create.next'))
    await type('#pool-name', 'tank')
    await submit('[role="dialog"] form')
    expect(document.querySelector('[role="dialog"] [role="alert"]')?.textContent).toContain(
      'tank exists',
    )
    expect(heading()).toContain(t('storage.create.stepOf', { current: 2, total: 3 }))
    expect(document.querySelector('#pool-confirm')).toBeNull()
  })

  it('reviews the plan, goes back to the name and plans again', async () => {
    on('POST', '/api/v1/storage/pools/plan', PLAN)
    on('POST', '/api/v1/storage/pools', problem(409, 'refused', 'plan expired'))
    await mountDialog()
    await choose('sdc', 'sde')
    await click(t('storage.create.next'))
    await type('#pool-name', 'tank')
    await submit('[role="dialog"] form')
    expect(heading()).toContain(t('storage.create.stepOf', { current: 3, total: 3 }))
    expect(page()).toContain('/volume1')
    const submitButton = button(t('storage.create.submit'))
    expect(submitButton.disabled).toBe(true)
    await type('#pool-confirm', 'tank')
    expect(submitButton.disabled).toBe(false)

    // A refused pool keeps the review open with the reason.
    await submit('#pool-create-form')
    expect(page()).toContain('plan expired')
    expect(heading()).toContain(t('storage.create.steps.review'))

    await click(t('desktop.back'))
    expect(heading()).toContain(t('storage.create.steps.name'))
    expect(page()).not.toContain('plan expired')
    expect(document.querySelector<HTMLInputElement>('#pool-name')!.value).toBe('tank')
    await submit('[role="dialog"] form')
    expect(called('POST', '/api/v1/storage/pools/plan')).toHaveLength(2)
    expect(document.querySelector<HTMLInputElement>('#pool-confirm')!.value).toBe('')
  })

  it('does not create a pool before its name is typed', async () => {
    on('POST', '/api/v1/storage/pools/plan', PLAN)
    await mountDialog()
    await choose('sdc', 'sde')
    await click(t('storage.create.next'))
    await type('#pool-name', 'tank')
    await submit('[role="dialog"] form')
    await type('#pool-confirm', 'tan')
    await submit('#pool-create-form')
    expect(called('POST', '/api/v1/storage/pools')).toHaveLength(0)
  })
})
