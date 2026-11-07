/* eslint-disable vue/one-component-per-file -- stand-ins for the app and its host */
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

import { mountApp, press, settle } from '@/desktop/testing/mount'
import { i18n } from '@/shared/i18n'
import { useOpenApp } from '@/shared/lib/window-section'

import { type AppDefinition, findApp } from './apps'
import AppWindow from './AppWindow.vue'
import { useWindows } from './windows'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string) => i18n.global.t(key)

// jsdom does not capture pointers.
HTMLElement.prototype.setPointerCapture = () => undefined

/** How the app in the window opens another, as it got it. */
let openApp: ReturnType<typeof useOpenApp> = null

const app: AppDefinition = {
  ...findApp('files')!,
  component: defineComponent({
    setup() {
      openApp = useOpenApp()
      return () => h('p', 'the app')
    },
  }),
}

/** A pointer event at `x`, `y`. */
function at(type: string, x: number, y: number): MouseEvent {
  return new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: x,
    clientY: y,
  })
}

async function opened(compact = false) {
  const Host = defineComponent({
    setup() {
      const windows = useWindows()
      windows.resized({ width: 1600, height: 1000 })
      windows.open(app)
      return () => {
        const window = windows.state.windows.find((entry) => entry.appId === app.id)
        return window ? h(AppWindow, { app, window, layer: 0, active: true, compact }) : null
      }
    },
  })
  const { wrapper } = await mountApp(Host)
  const windows = useWindows()
  const window = () => windows.state.windows.find((entry) => entry.appId === app.id)!
  return { wrapper: wrapper.findComponent(AppWindow), windows, window }
}

describe('a window', () => {
  it('moves by its title bar and grows by its edges', async () => {
    const { wrapper, window } = await opened()
    const before = { ...window().geometry }
    const bar = wrapper.find('header').exists()
      ? wrapper.find('header').element
      : wrapper.element.children[0]!
    bar.dispatchEvent(at('pointerdown', 100, 100))
    bar.dispatchEvent(at('pointermove', 150, 130))
    bar.dispatchEvent(at('pointerup', 150, 130))
    await settle()
    expect(window().geometry.x).toBe(before.x + 50)
    expect(window().geometry.y).toBe(before.y + 30)

    const edges = wrapper.findAll('.touch-none').map((edge) => edge.element)
    for (const edge of edges) {
      edge.dispatchEvent(at('pointerdown', 200, 200))
      edge.dispatchEvent(at('pointermove', 240, 260))
      edge.dispatchEvent(at('pointercancel', 240, 260))
    }
    await settle()
    expect(window().geometry.width).toBeGreaterThan(before.width)
    expect(window().geometry.height).toBeGreaterThan(before.height)
  })

  it('maximizes on a double click and does not move then', async () => {
    const { wrapper, window } = await opened()
    const bar = wrapper.element.children[0]!
    bar.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))
    await settle()
    expect(window().maximized).toBe(true)
    // Without a size of its own, it fills the desktop rather than collapse.
    const section = wrapper.element as HTMLElement
    expect(section.classList).toContain('inset-0')
    expect(section.style.width).toBe('')
    const x = window().geometry.x
    bar.dispatchEvent(at('pointerdown', 10, 10))
    bar.dispatchEvent(at('pointermove', 90, 90))
    expect(window().geometry.x).toBe(x)
  })

  it('goes back full screen on a phone', async () => {
    const { wrapper, window } = await opened(true)
    await settle()
    expect(wrapper.text()).toContain('the app')
    await press(t('desktop.back'))
    expect(window().minimized).toBe(true)
  })

  it('lets its app open another one on a page', async () => {
    const { windows } = await opened()
    await settle()

    openApp!('docker', 'project:media')
    openApp!('no-such-app')

    const apps = windows.state.windows.map((entry) => [entry.appId, entry.section])
    expect(apps).toEqual([
      ['files', undefined],
      ['docker', 'project:media'],
    ])
  })
})
