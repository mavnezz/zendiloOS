import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import {
  enableAutoUnmount,
  flushPromises,
  mount,
  type ComponentMountingOptions,
} from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach } from 'vitest'
import type { Component } from 'vue'

import { i18n } from '@/shared/i18n'

// jsdom has no ResizeObserver; charts and editors only ask for sizes.
if (typeof globalThis.ResizeObserver === 'undefined') {
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
}

/** What the screen is like for `matchMedia`: a phone when `compact`. */
export const screen = { compact: false, dark: false }

// jsdom has no matchMedia either.
window.matchMedia = (query: string) => ({
  matches: query.includes('max-width')
    ? screen.compact
    : query.includes('dark')
      ? screen.dark
      : false,
  media: query,
  onchange: null,
  addEventListener: () => undefined,
  removeEventListener: () => undefined,
  addListener: () => undefined,
  removeListener: () => undefined,
  dispatchEvent: () => false,
})

// Each test's components go after it, then whatever is left in the
// document (hooks after a test run last registered first).
afterEach(() => {
  document.body.innerHTML = ''
})
enableAutoUnmount(afterEach)

export type Options<C extends Component> = ComponentMountingOptions<C> & {
  /** Runs once Pinia is active, before mounting: to sign in, say. */
  before?: () => void
}

/**
 * Mounts `component` as the app has it: i18n, Pinia and a query client
 * that does not ask again; attached to the document, so dialogs that
 * teleport there can be found.
 */
export async function mountApp<C extends Component>(component: C, options: Options<C> = {}) {
  i18n.global.locale.value = 'en'
  const pinia = createPinia()
  setActivePinia(pinia)
  options.before?.()
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const { global, ...rest } = options
  delete (rest as { before?: unknown }).before
  const wrapper = mount(component, {
    attachTo: document.body,
    ...rest,
    global: {
      ...global,
      plugins: [i18n, pinia, [VueQueryPlugin, { queryClient }], ...(global?.plugins ?? [])],
    },
  })
  await settle()
  return { wrapper, queryClient }
}

/** Lets requests answer and the page follow. */
export async function settle(): Promise<void> {
  for (let round = 0; round < 3; round += 1) {
    await flushPromises()
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
}

/** The document's text: dialogs teleport out of the wrapper. */
export function page(): string {
  return document.body.textContent ?? ''
}

/** A button anywhere in the document, by its text (or a pattern of it). */
export function button(text: string | RegExp): HTMLButtonElement {
  const found = [...document.body.querySelectorAll<HTMLButtonElement>('button')].find(
    (candidate) =>
      typeof text === 'string'
        ? candidate.textContent?.trim() === text
        : text.test(candidate.textContent ?? ''),
  )
  if (!found) throw new Error(`no button ${String(text)} in: ${page().slice(0, 600)}`)
  return found
}

/** A button anywhere in the document, by its accessible label. */
export function labelled(label: string): HTMLButtonElement {
  const found = document.body.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)
  if (!found) throw new Error(`no button labelled ${label}`)
  return found
}

export async function press(label: string): Promise<void> {
  labelled(label).click()
  await settle()
}

export async function click(text: string | RegExp): Promise<void> {
  button(text).click()
  await settle()
}

/** Types into the input, select or textarea `selector` (e.g. `#pool-name`). */
export async function type(selector: string, value: string): Promise<void> {
  const element = document.body.querySelector<
    HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement
  >(selector)
  if (!element) throw new Error(`no ${selector}`)
  element.value = value
  element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? 'change' : 'input'))
  await settle()
}

/** Submits the first form in the document (or the one `selector` names). */
export async function submit(selector = 'form'): Promise<void> {
  document.body.querySelector(selector)?.dispatchEvent(new Event('submit', { cancelable: true }))
  await settle()
}
