/**
 * Component tests against a stand-in server (see `./fake-fetch`), mounted
 * as the app mounts them. Import this before anything that imports the API
 * client: the first import below puts the stand-in in place of `fetch`.
 */
import './fake-fetch'

import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { type ComponentMountingOptions, mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import type { Component } from 'vue'

import { i18n } from '@/shared/i18n'

import { resetServer } from './fake-fetch'

export { answer, type Call, calls, callsTo, on, problem, resetServer } from './fake-fetch'

const mounted: { unmount(): void }[] = []

/** Unmounts what was mounted, forgets the handlers and requests, empties the page. */
export function cleanup(): void {
  for (const wrapper of mounted.splice(0)) wrapper.unmount()
  resetServer()
  document.body.innerHTML = ''
}

/** Mounts `component` as the app has it: Pinia, i18n (English), vue-query without retries. */
export function mountApp<T extends Component>(
  component: T,
  options: ComponentMountingOptions<T> = {},
) {
  const pinia = createPinia()
  setActivePinia(pinia)
  i18n.global.locale.value = 'en'
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const { global, ...rest } = options as ComponentMountingOptions<T> & {
    global?: { plugins?: unknown[] }
  }
  const wrapper = mount(component, {
    attachTo: document.body,
    ...rest,
    global: {
      ...global,
      plugins: [pinia, i18n, [VueQueryPlugin, { queryClient }], ...(global?.plugins ?? [])],
    },
  })
  mounted.push(wrapper)
  return wrapper
}

/** Lets requests, queries and renders settle. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
}

/** The buttons in the document (dialogs are teleported to its body) with this text. */
export function buttons(text: string): HTMLButtonElement[] {
  return [...document.body.querySelectorAll('button')].filter((b) => b.textContent?.trim() === text)
}

/** Clicks the only button with this text in the document. */
export async function click(text: string): Promise<void> {
  const found = buttons(text)
  if (found.length !== 1)
    throw new Error(`${found.length} buttons "${text}": ${document.body.innerHTML.slice(0, 2000)}`)
  found[0]!.click()
  await settle()
}

/** Types into an input or text area found in the document. */
export async function type(selector: string, value: string): Promise<void> {
  const element = document.body.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector)
  if (!element) throw new Error(`no ${selector}`)
  element.value = value
  element.dispatchEvent(new Event('input', { bubbles: true }))
  element.dispatchEvent(new Event('change', { bubbles: true }))
  await settle()
}

/** Chooses `value` in a select found in the document. */
export async function choose(selector: string, value: string): Promise<void> {
  const element = document.body.querySelector<HTMLSelectElement>(selector)
  if (!element) throw new Error(`no ${selector}`)
  element.value = value
  element.dispatchEvent(new Event('change', { bubbles: true }))
  await settle()
}

/** Opens the menu behind the control labelled `label` and picks `item` in it. */
export async function pick(label: string, item: string): Promise<void> {
  const trigger = document.body.querySelector<HTMLElement>(`[aria-label="${label}"]`)
  if (!trigger) throw new Error(`no menu "${label}"`)
  trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
  await settle()
  const items = [...document.body.querySelectorAll<HTMLElement>('[role="menuitem"]')]
  const found = items.find((i) => i.textContent?.trim() === item)
  if (!found)
    throw new Error(`no "${item}" among ${items.map((i) => i.textContent?.trim()).join(', ')}`)
  found.click()
  await settle()
}

/** Sets the checkbox inside the label that says `text`. */
export async function tick(text: string, on = true): Promise<void> {
  const label = [...document.body.querySelectorAll<HTMLLabelElement>('label')].find((l) =>
    l.textContent?.includes(text),
  )
  const box = label?.querySelector<HTMLInputElement>('input[type="checkbox"]')
  if (!box) throw new Error(`no checkbox "${text}"`)
  if (box.checked !== on) box.click()
  await settle()
}

/** Clicks the element labelled `label` (a checkbox, an icon button). */
export async function press(label: string): Promise<void> {
  const element = document.body.querySelector<HTMLElement>(`[aria-label="${label}"]`)
  if (!element) throw new Error(`nothing labelled "${label}"`)
  element.click()
  await settle()
}

/** The text of the page, dialogs included. */
export function page(): string {
  return document.body.textContent ?? ''
}
