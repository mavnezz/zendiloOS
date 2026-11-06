import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it } from 'vitest'

import ConfirmDialog from './ConfirmDialog.vue'

afterEach(() => {
  document.body.innerHTML = ''
})

describe('the confirm dialog', () => {
  it('says confirm before it closes, so the caller still knows what was asked', async () => {
    const order: string[] = []
    const wrapper = mount(ConfirmDialog, {
      props: {
        open: true,
        title: 'Restart?',
        description: 'It restarts.',
        confirmLabel: 'Restart',
        cancelLabel: 'Cancel',
        onConfirm: () => order.push('confirm'),
        'onUpdate:open': (open: boolean) => order.push(`open:${open}`),
      },
      attachTo: document.body,
    })
    await flushPromises()
    const confirm = [...document.body.querySelectorAll('button')].find(
      (b) => b.textContent?.trim() === 'Restart',
    )
    confirm!.click()
    await flushPromises()
    expect(order).toEqual(['confirm', 'open:false'])
    wrapper.unmount()
  })
})
