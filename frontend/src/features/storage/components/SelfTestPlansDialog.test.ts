import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { Disk, SelfTestPlan, SelfTestPlans } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'

import SelfTestPlansDialog from './SelfTestPlansDialog.vue'

const plans = ref<SelfTestPlans | undefined>()
const disks = ref<Partial<Disk>[]>([])
const saveSelfTestPlans = vi.fn<(plans: SelfTestPlan[]) => Promise<void>>(() => Promise.resolve())

vi.mock('../api', () => ({
  useSelfTestPlans: () => ({ data: plans, error: ref(null) }),
  useDisks: () => ({ data: disks }),
  useRefreshStorage: () => () => Promise.resolve(),
  saveSelfTestPlans: (body: SelfTestPlan[]) => saveSelfTestPlans(body),
}))

function mountDialog() {
  return mount(SelfTestPlansDialog, {
    props: { open: true },
    global: { plugins: [i18n] },
    attachTo: document.body,
  })
}

/** The dialog renders into a portal on the page. */
function page(): HTMLElement {
  return document.body
}

function checkbox(label: string): HTMLInputElement {
  const found = [...page().querySelectorAll('label')].find((l) => l.textContent?.includes(label))
  const input = found?.querySelector('input')
  if (!input) throw new Error(`no input labelled "${label}"`)
  return input
}

async function submit(): Promise<void> {
  page().querySelector('form')?.dispatchEvent(new Event('submit'))
  await flushPromises()
}

beforeEach(() => {
  i18n.global.locale.value = 'en'
  saveSelfTestPlans.mockClear()
  disks.value = [
    { name: 'sda', model: 'WD RED', serial: 'WD-1' },
    { name: 'sdb', model: 'WD RED', serial: 'WD-2' },
  ]
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the self-test schedule', () => {
  it('starts from the server’s plans and says when they last ran', async () => {
    plans.value = {
      utc_offset_seconds: 3600,
      plans: [
        {
          kind: 'long',
          frequency: 'monthly',
          hour: 3,
          disks: ['WD-2', 'OLD-9'],
          last_run_at: null,
        },
      ],
    }
    const wrapper = mountDialog()
    await flushPromises()

    expect(checkbox('Short test').checked).toBe(false)
    expect(checkbox('Long test').checked).toBe(true)
    expect(checkbox('sdb').checked).toBe(true)
    expect(checkbox('sda').checked).toBe(false)
    expect(page().textContent).toContain('OLD-9 (not connected)')
    expect(page().textContent).toContain('Not started yet')
    expect(page().textContent).toContain('UTC+01:00')
    wrapper.unmount()
  })

  it('saves the plans switched on, all disks meaning none named', async () => {
    plans.value = { utc_offset_seconds: 0, plans: [] }
    const wrapper = mountDialog()
    await flushPromises()

    checkbox('Short test').click()
    await flushPromises()
    await submit()

    expect(saveSelfTestPlans).toHaveBeenCalledWith([
      { kind: 'short', frequency: 'weekly', hour: 2, disks: [] },
    ])
    wrapper.unmount()
  })
})
