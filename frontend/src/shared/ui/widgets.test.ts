import { mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { h, nextTick, ref } from 'vue'

import { i18n } from '@/shared/i18n'

import { CardFooter } from './card'
import { TimeChart } from './chart'
import { CodeEditor } from './code-editor'
import { AppFrame, PanelTabs } from './frame'
import { Gauge } from './gauge'
import { InfoList } from './info-list'
import { PageCard, PageDescription } from './page'
import { Sparkline } from './sparkline'

/** ResizeObserver as the chart uses it: reports a width once observing. */
class Sizes {
  constructor(private readonly report: (entries: { contentRect: { width: number } }[]) => void) {}
  observe(): void {
    this.report([{ contentRect: { width: 400 } }])
  }
  disconnect(): void {}
}

afterEach(() => vi.unstubAllGlobals())

describe('the time chart', () => {
  function chart(props: Record<string, unknown> = {}) {
    vi.stubGlobal('ResizeObserver', Sizes)
    i18n.global.locale.value = 'en'
    return mount(TimeChart, {
      props: {
        capacity: 60,
        series: [
          { label: 'Read', values: [1, 4, 2, 8], color: 'red' },
          { label: 'Write', values: [3, 1], color: 'blue', dashed: true },
        ],
        ...props,
      },
      global: { plugins: [i18n] },
    })
  }

  it('draws both series with a scale, a time axis and a legend', async () => {
    const wrapper = chart({ format: (value: number) => `${value} MB` })
    await nextTick()

    expect(wrapper.find('svg').attributes('aria-label')).toBe('Read, Write')
    expect(wrapper.findAll('path')).toHaveLength(3)
    expect(wrapper.text()).toContain('MB')
    expect(wrapper.text()).toContain('now')
    expect(wrapper.find('figcaption').text()).toContain('Write')
    wrapper.unmount()
  })

  it('shows the values under the pointer, and hides them again', async () => {
    const wrapper = chart({ capacity: 4, interval: 5, max: 10, base: 1024, legend: false })
    await nextTick()
    const svg = wrapper.find('svg')
    const point = async (clientX: number) => {
      svg.element.dispatchEvent(new MouseEvent('pointermove', { clientX }))
      await nextTick()
    }

    await point(400)
    expect(wrapper.text()).toContain('now')
    expect(wrapper.findAll('circle')).toHaveLength(2)
    await point(52)
    expect(wrapper.text()).toContain('–')
    await point(-500)
    expect(wrapper.findAll('circle')).toHaveLength(0)
    await point(240)
    await svg.trigger('pointerleave')
    expect(wrapper.findAll('circle')).toHaveLength(0)
    expect(wrapper.find('figcaption').exists()).toBe(false)
  })

  it('labels a long history every two minutes', async () => {
    const wrapper = chart({
      capacity: 300,
      interval: 2,
      series: [{ label: 'CPU', values: [], color: 'red' }],
    })
    await nextTick()
    expect(wrapper.text()).toContain('−10 min')
  })
})

describe('small views', () => {
  it('draw a sparkline from a short history, starting at the right', () => {
    const line = mount(Sparkline, {
      props: { values: [1, 2, 3], capacity: 5, max: 4, class: 'text-red-500' },
    })
    expect(line.find('polyline').attributes('points')).toContain('50.00')
    expect(line.find('polygon').exists()).toBe(true)
    const point = mount(Sparkline, { props: { values: [2] } })
    expect(point.find('polyline').exists()).toBe(false)
  })

  it('fill a gauge, red when almost full, and never past its ends', () => {
    const full = mount(Gauge, { props: { value: 0.95, label: 'CPU', valueText: '95 %' } })
    expect(full.attributes('aria-valuenow')).toBe('95')
    expect(full.html()).toContain('text-destructive')
    expect(mount(Gauge, { props: { value: 0.8, label: 'CPU' } }).html()).toContain('text-warning')
    expect(
      mount(Gauge, { props: { value: Number.NaN, label: 'CPU' } }).attributes('aria-valuenow'),
    ).toBe('0')
    expect(
      mount(Gauge, { props: { value: 3, label: 'CPU', size: 40 } }).attributes('aria-valuenow'),
    ).toBe('100')
  })

  it("fold a page's help away, on every page at once", async () => {
    const options = { global: { plugins: [i18n] }, slots: { default: 'What this page does.' } }
    const one = mount(PageDescription, options)
    const other = mount(PageDescription, options)
    expect(one.find('button').attributes('aria-expanded')).toBe('true')

    await one.find('button').trigger('click')

    expect(other.find('button').attributes('aria-expanded')).toBe('false')
    expect(localStorage.getItem('zendiloos.help')).toBe('closed')
    await other.find('button').trigger('click')
    expect(one.find('button').attributes('aria-expanded')).toBe('true')
    expect(localStorage.getItem('zendiloos.help')).toBe('open')
  })

  it('list names and values', () => {
    const items = [
      { label: 'Kernel', value: '6.12', mono: true },
      { label: 'Host', value: 'nas' },
    ]
    expect(mount(InfoList, { props: { items } }).text()).toContain('Kernel6.12')
    expect(mount(InfoList, { props: { items } }).find('dd').attributes('title')).toBe('6.12')
    expect(mount(CardFooter, { slots: { default: 'Done' } }).text()).toBe('Done')
    expect(
      mount(PageCard, { props: { title: 'Disks', description: 'All of them' } }).text(),
    ).toContain('All of them')
  })

  it('switch panel tabs, with their counts', async () => {
    const active = ref('a')
    const tabs = mount({
      render: () =>
        h(PanelTabs, {
          tabs: [
            { id: 'a', label: 'One' },
            { id: 'b', label: 'Two', count: 3 },
          ],
          modelValue: active.value,
          'onUpdate:modelValue': (value: string) => (active.value = value),
        }),
    })
    expect(tabs.text()).toContain('3')
    await tabs.findAll('button')[1]!.trigger('click')
    expect(active.value).toBe('b')
  })

  it('give an app a navigation that chooses its section', async () => {
    const section = ref('one')
    const frame = mount({
      render: () =>
        h(
          AppFrame,
          {
            groups: [
              {
                id: 'g',
                label: 'General',
                items: [
                  { id: 'one', label: 'One', count: 2 },
                  { id: 'two', label: 'Two', alert: true },
                ],
              },
            ],
            section: section.value,
            'onUpdate:section': (value: string) => (section.value = value),
          },
          { default: () => 'content' },
        ),
    })
    expect(frame.find('nav').attributes('aria-label')).toBe('One')
    await frame.findAll('nav button')[1]!.trigger('click')
    expect(section.value).toBe('two')
  })

  it('edit text with line numbers, indenting on tab', async () => {
    const text = ref('services:\napp:')
    const frames: FrameRequestCallback[] = []
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) =>
      frames.push(callback),
    )
    const editor = mount({
      render: () =>
        h(CodeEditor, {
          label: 'Compose file',
          modelValue: text.value,
          'onUpdate:modelValue': (value: string) => (text.value = value),
        }),
    })
    expect(editor.findAll('[aria-hidden="true"] div')).toHaveLength(2)
    const area = editor.find('textarea')
    ;(area.element as HTMLTextAreaElement).setSelectionRange(10, 10)
    await area.trigger('keydown', { key: 'Tab' })
    expect(text.value).toBe('services:\n  app:')
    frames.forEach((frame) => frame(0))
    await area.trigger('keydown', { key: 'Tab', shiftKey: true })
    expect(text.value).toBe('services:\n  app:')
    await area.trigger('scroll')
  })
})
