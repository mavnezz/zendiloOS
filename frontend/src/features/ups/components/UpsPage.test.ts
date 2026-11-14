import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { UpsOverview, UpsReading, UpsSettings } from '@/shared/api/types'
import { i18n } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { button, click, mountApp, page, settle, type } from '@/shared/test-support/mount'
import { called, on, problem, reset } from '@/shared/test-support/server'

import UpsPage from './UpsPage.vue'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/shared/test-support/server')).routed(await original()),
)

const t = (key: string, values: Record<string, unknown> = {}) => i18n.global.t(key, values)

const NONE: UpsSettings = {
  source: null,
  shutdown: { kind: 'low_battery' },
  cut_power: false,
  serve: null,
}
const USB: UpsSettings = {
  ...NONE,
  source: { kind: 'usb', driver: 'usbhid-ups', vendor_id: '0463', product_id: 'ffff' },
}
const READING: UpsReading = {
  status: 'OL CHRG',
  on_battery: false,
  low_battery: false,
  charging: true,
  replace_battery: false,
  charge_percent: 87,
  runtime_seconds: 1500,
  load_percent: 23,
  manufacturer: 'EATON',
  model: 'Ellipse ECO 650',
}
const overview = (settings: UpsSettings = NONE, rest: Partial<UpsOverview> = {}): UpsOverview => ({
  installed: true,
  snmp: true,
  settings,
  reading: null,
  drivers: ['usbhid-ups', 'nutdrv_qx', 'blazer_usb'],
  name: 'ups',
  port: 3493,
  default_account: 'monuser',
  ...rest,
})
const JOB = {
  id: 7,
  kind: 'ups.install',
  target: 'nut',
  state: 'running',
  created_at: 1,
  updated_at: 1,
}

const apply = () => click(t('common.apply'))
const sent = () => called('PUT', '/api/v1/ups').at(-1)?.body
const press = async (selector: string) => {
  document.querySelector<HTMLButtonElement>(selector)!.click()
  await settle()
}
const text = (id: string) => document.querySelector(`[data-testid="${id}"]`)?.textContent ?? ''

beforeEach(() => {
  reset()
  on('GET', '/api/v1/ups', overview())
  // What is set comes back as what is kept.
  on('PUT', '/api/v1/ups', overview(USB, { reading: READING }))
})

describe('the UPS page', () => {
  it('installs the Network UPS Tools, says how it goes, then shows the settings', async () => {
    on('GET', '/api/v1/ups', overview(USB, { installed: false, snmp: false }))
    on('POST', '/api/v1/ups/install', JOB)
    await mountApp(UpsPage, { props: { canManage: true } })
    expect(page()).toContain(t('ups.install.title'))
    expect(page()).toContain(t('ups.install.waiting'))
    expect(document.querySelector('#ups-source')).toBeNull()

    await click(t('ups.install.action'))
    expect(called('POST', '/api/v1/ups/install')).toHaveLength(1)
    expect(page()).toContain(t('ups.install.running'))

    const jobs = useJobsStore()
    jobs.update({
      ...JOB,
      state: 'failed',
      error_detail: 'E: Unable to locate package nut',
    } as never)
    await settle()
    expect(page()).toContain(t('ups.install.failed'))
    expect(page()).toContain('Unable to locate package nut')

    on('GET', '/api/v1/ups', overview(USB, { reading: READING }))
    jobs.update({ ...JOB, id: 8, state: 'running' } as never)
    await settle()
    jobs.update({ ...JOB, id: 8, state: 'succeeded' } as never)
    await settle()
    expect(page()).not.toContain(t('ups.install.title'))
    expect(document.querySelector<HTMLSelectElement>('#ups-source')?.value).toBe('usb')
  })

  it('says why an installation did not start, and offers none to those who may only look', async () => {
    on('GET', '/api/v1/ups', overview(NONE, { installed: false }))
    on('POST', '/api/v1/ups/install', problem(409, 'update_running'))
    const { wrapper } = await mountApp(UpsPage, { props: { canManage: true } })
    await click(t('ups.install.action'))
    expect(page()).toContain(t('errors.update_running'))
    await wrapper.setProps({ canManage: false })
    expect(page()).not.toContain(t('ups.install.action'))
  })

  it('finds the UPS on USB and follows it', async () => {
    on('POST', '/api/v1/ups/scan', {
      devices: [
        {
          driver: 'usbhid-ups',
          vendor_id: '0463',
          product_id: 'FFFF',
          vendor: 'EATON',
          product: 'Ellipse ECO',
          serial: null,
          supported: true,
        },
      ],
    })
    await mountApp(UpsPage, { props: { canManage: true } })
    // Without a UPS there is nothing to say about a shutdown, and no state.
    expect(page()).toContain(t('ups.source.hints.none'))
    expect(page()).not.toContain(t('ups.shutdown.label'))
    expect(page()).not.toContain(t('ups.status.title'))

    await type('#ups-source', 'usb')
    expect(text('ups-device')).toContain(t('ups.usb.any'))
    await click(t('ups.usb.detect'))
    // The one found is taken, its ids as NUT writes them.
    expect(text('ups-device')).toContain('0463:ffff')
    expect(text('ups-found')).toContain('EATON Ellipse ECO')
    expect(document.querySelector('[data-testid="ups-found"] button')?.ariaPressed).toBe('true')

    await press('[role="radio"][value="after_minutes"]')
    await type('#ups-minutes', '5')
    await press('#ups-cut')
    await apply()
    expect(sent()).toEqual({
      source: {
        kind: 'usb',
        driver: 'usbhid-ups',
        vendor_id: '0463',
        product_id: 'ffff',
        serial: null,
      },
      shutdown: { kind: 'after_minutes', minutes: 5 },
      cut_power: true,
      serve: null,
    })
    // What came back is shown: nothing left to apply, and the UPS's state.
    expect(page()).not.toContain(t('common.apply'))
    expect(text('ups-state')).toContain(t('ups.status.onMains'))
    expect(text('ups-state')).toContain(t('ups.status.charging'))
    expect(page()).toContain('EATON Ellipse ECO 650')
    expect(page()).toContain('87 %')
    expect(page()).toContain('25 min')
    expect(page()).toContain('OL CHRG')
  })

  it('offers what the scan found to choose from, and any UPS again', async () => {
    const device = (product: string, id: string, driver = 'usbhid-ups') => ({
      driver,
      vendor_id: '051d',
      product_id: id,
      vendor: 'APC',
      product,
      serial: `S${id}`,
      supported: driver === 'usbhid-ups',
    })
    on('GET', '/api/v1/ups', overview(USB))
    on('POST', '/api/v1/ups/scan', {
      devices: [
        device('Back-UPS 700', '0002'),
        device('Smart-UPS', '0003'),
        device('Odd one', '0004', 'apcupsd-ups'),
        { driver: 'nutdrv_qx', supported: true },
      ],
    })
    await mountApp(UpsPage, { props: { canManage: true } })
    expect(text('ups-device')).toContain('0463:ffff')
    await click(t('ups.usb.detect'))
    // Several: none is taken unasked.
    expect(text('ups-device')).toContain('0463:ffff')
    expect(text('ups-found')).toContain(t('ups.usb.unnamed'))
    expect(button(/Odd one/).disabled).toBe(true)
    expect(text('ups-found')).toContain(t('ups.usb.unsupported', { driver: 'apcupsd-ups' }))
    await click(/Smart-UPS/)
    expect(text('ups-device')).toContain('051d:0003')
    expect(text('ups-device')).toContain('S0003')
    await click(t('ups.usb.forget'))
    expect(text('ups-device')).toContain(t('ups.usb.any'))
    await type('#ups-driver', 'nutdrv_qx')
    await apply()
    expect((sent() as { source: unknown }).source).toEqual({
      kind: 'usb',
      driver: 'nutdrv_qx',
      vendor_id: null,
      product_id: null,
      serial: null,
    })

    on('POST', '/api/v1/ups/scan', { devices: [] })
    await click(t('ups.usb.detect'))
    expect(page()).toContain(t('ups.usb.none'))
    on('POST', '/api/v1/ups/scan', problem(409, 'refused', 'the scan did not end'))
    await click(t('ups.usb.detect'))
    expect(page()).toContain('the scan did not end')
  })

  it('follows a UPS on the network over SNMP', async () => {
    on('GET', '/api/v1/ups', overview(NONE, { snmp: false }))
    on('POST', '/api/v1/ups/install', JOB)
    await mountApp(UpsPage, { props: { canManage: true } })
    await type('#ups-source', 'snmp')
    // NUT was installed by hand, without its SNMP driver: installed from here.
    expect(page()).toContain(t('errors.ups_snmp_missing'))
    expect(button(t('common.apply')).disabled).toBe(true)
    await click(t('ups.install.action'))
    expect(page()).toContain(t('ups.install.running'))
    on('GET', '/api/v1/ups', overview(NONE, { snmp: true }))
    useJobsStore().update({ ...JOB, state: 'succeeded' } as never)
    await settle()
    expect(page()).not.toContain(t('errors.ups_snmp_missing'))
    // Not without an address.
    expect(button(t('common.apply')).disabled).toBe(true)
    expect(page()).toContain(t('ups.problems.address'))
    await type('#ups-address', ' 10.0.0.9 ')
    await type('#ups-version', 'v1')
    await press('[role="radio"][value="charge_below"]')
    await type('#ups-percent', '3')
    expect(page()).toContain(t('ups.problems.percent'))
    await type('#ups-percent', '40')
    await apply()
    expect(sent()).toEqual({
      source: { kind: 'snmp', address: '10.0.0.9', community: 'public', version: 'v1' },
      shutdown: { kind: 'charge_below', percent: 40 },
      cut_power: false,
      serve: null,
    })
  })

  it('follows the UPS of another device, which alone can be told things', async () => {
    on(
      'GET',
      '/api/v1/ups',
      overview({ ...USB, shutdown: { kind: 'charge_below', percent: 40 }, cut_power: true }),
    )
    await mountApp(UpsPage, { props: { canManage: true } })
    await type('#ups-source', 'server')
    // A charge of its own and cutting the output need the UPS here.
    expect(
      document.querySelector('[role="radio"][value="low_battery"]')?.getAttribute('aria-checked'),
    ).toBe('true')
    expect(
      document.querySelector<HTMLButtonElement>('[role="radio"][value="charge_below"]')?.disabled,
    ).toBe(true)
    expect(document.querySelector('#ups-cut')).toBeNull()
    expect(document.querySelector('#ups-serve')).toBeNull()
    expect(page()).toContain(t('ups.shutdown.hintServer'))

    await type('#ups-host', 'nas.local')
    expect(page()).toContain(t('ups.problems.password'))
    await type('#ups-port', '0')
    expect(page()).toContain(t('ups.problems.port'))
    await type('#ups-port', '3493')
    await type('#ups-password', 'secret')
    await press('[role="radio"][value="after_minutes"]')
    await type('#ups-minutes', '0')
    expect(page()).toContain(t('ups.problems.minutes'))
    await type('#ups-minutes', '15')
    on(
      'PUT',
      '/api/v1/ups',
      overview({
        ...NONE,
        source: {
          kind: 'server',
          host: 'nas.local',
          port: 3493,
          ups: 'ups',
          user: 'monuser',
          password_set: true,
        },
        shutdown: { kind: 'after_minutes', minutes: 15 },
      }),
    )
    await apply()
    expect(sent()).toEqual({
      source: {
        kind: 'server',
        host: 'nas.local',
        port: 3493,
        ups: 'ups',
        user: 'monuser',
        password: 'secret',
      },
      shutdown: { kind: 'after_minutes', minutes: 15 },
      cut_power: false,
      serve: null,
    })
    // Kept: changed without typing the password again, unless the account changes.
    expect(page()).toContain(t('ups.status.silent'))
    expect(document.querySelector<HTMLInputElement>('#ups-password')?.placeholder).toBe(
      t('ups.passwordKept'),
    )
    await type('#ups-name', 'qnapups')
    await apply()
    expect((sent() as { source: { password: unknown } }).source.password).toBeNull()
    await type('#ups-user', 'other')
    expect(page()).toContain(t('ups.problems.password'))
  })

  it('passes the state on to other devices and says what they are given', async () => {
    on('GET', '/api/v1/ups', overview(USB, { reading: READING }))
    await mountApp(UpsPage, { props: { canManage: true, address: '192.168.1.127' } })
    await press('#ups-serve')
    expect(page()).toContain(t('ups.problems.password'))
    expect(text('ups-for-others')).toContain('192.168.1.127')
    expect(text('ups-for-others')).toContain('3493')
    expect(text('ups-for-others')).toContain('monuser')
    expect(text('ups-for-others')).toContain(t('ups.serve.firewall', { port: 3493 }))
    await type('#ups-serve-user', '')
    expect(page()).toContain(t('ups.problems.account'))
    await type('#ups-serve-user', 'follower')
    await type('#ups-serve-password', 'for-others')
    on(
      'PUT',
      '/api/v1/ups',
      overview({ ...USB, serve: { user: 'follower', password_set: true } }, { reading: READING }),
    )
    await apply()
    expect((sent() as { serve: unknown }).serve).toEqual({
      user: 'follower',
      password: 'for-others',
    })
    expect(document.querySelector<HTMLInputElement>('#ups-serve-password')?.placeholder).toBe(
      t('ups.passwordKept'),
    )
  })

  it('tells a power cut, a battery about to give out, and a UPS gone silent', async () => {
    const reading = (status: Partial<UpsReading>) =>
      overview(USB, { reading: { ...READING, charging: false, ...status } })
    on('GET', '/api/v1/ups', reading({ on_battery: true, status: 'OB DISCHRG' }))
    const { queryClient } = await mountApp(UpsPage, { props: { canManage: false } })
    expect(text('ups-state')).toBe(t('ups.status.onBattery'))

    on(
      'GET',
      '/api/v1/ups',
      reading({
        on_battery: true,
        low_battery: true,
        replace_battery: true,
        charge_percent: null,
        runtime_seconds: null,
        load_percent: null,
      }),
    )
    await queryClient.invalidateQueries({ queryKey: ['ups'] })
    await settle()
    expect(text('ups-state')).toContain(t('ups.status.lowBattery'))
    expect(text('ups-state')).toContain(t('ups.status.replaceBattery'))
    expect(page()).not.toContain('87 %')

    on('GET', '/api/v1/ups', overview(USB))
    await queryClient.invalidateQueries({ queryKey: ['ups'] })
    await settle()
    expect(page()).toContain(t('ups.status.silent'))
    // Who may only look sees the settings and changes none.
    expect(document.querySelector<HTMLSelectElement>('#ups-source')?.disabled).toBe(true)
    expect(page()).not.toContain(t('ups.usb.detect'))
  })

  it('says what the system refused and takes a change back', async () => {
    on('PUT', '/api/v1/ups', problem(409, 'refused', 'the driver is missing'))
    await mountApp(UpsPage, { props: { canManage: true } })
    await type('#ups-source', 'usb')
    await apply()
    expect(page()).toContain('the driver is missing')
    await click(t('common.reset'))
    expect(document.querySelector<HTMLSelectElement>('#ups-source')?.value).toBe('none')
    expect(page()).not.toContain('the driver is missing')
  })

  it('shows a simulated UPS as what it is, and a failure to load', async () => {
    on('GET', '/api/v1/ups', overview({ ...NONE, source: { kind: 'simulated' } }))
    const mounted = await mountApp(UpsPage, { props: { canManage: true } })
    expect(document.querySelector<HTMLSelectElement>('#ups-source')?.value).toBe('simulated')
    expect(page()).toContain(t('ups.source.hints.simulated'))
    await press('#ups-serve')
    await type('#ups-serve-password', 'x')
    on('PUT', '/api/v1/ups', overview({ ...NONE, source: { kind: 'simulated' } }))
    await apply()
    expect((sent() as { source: unknown }).source).toEqual({ kind: 'simulated' })
    mounted.wrapper.unmount()

    on('GET', '/api/v1/ups', problem(403, 'forbidden'))
    await mountApp(UpsPage, { props: { canManage: false } })
    expect(page()).toContain(t('errors.forbidden'))
  })
})
