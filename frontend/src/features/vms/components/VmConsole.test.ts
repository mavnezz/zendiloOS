import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '@/shared/i18n'
import { button, click, mountApp, page, settle } from '@/shared/test-support/mount'

import VmConsole from './VmConsole.vue'

const t = (key: string) => i18n.global.t(key)

/** noVNC's client, pretended: what it was given and asked. */
const clients: FakeRfb[] = []

class FakeRfb extends EventTarget {
  scaleViewport = false
  resizeSession = true
  viewOnly = false
  focusOnClick = true
  background = ''
  sendCtrlAltDel = vi.fn()
  focus = vi.fn()
  disconnect = vi.fn(() => this.dispatchEvent(new CustomEvent('disconnect')))
  constructor(
    readonly target: HTMLElement,
    readonly channel: FakeSocket,
  ) {
    super()
    clients.push(this)
  }
}

vi.mock('@novnc/novnc', () => ({ default: FakeRfb }))

class FakeSocket extends EventTarget {
  static opened: FakeSocket[] = []
  constructor(
    readonly url: string,
    readonly protocols: string[],
  ) {
    super()
    FakeSocket.opened.push(this)
  }
  close(code = 1000, reason = ''): void {
    this.dispatchEvent(Object.assign(new Event('close'), { code, reason }))
  }
}

beforeEach(() => {
  clients.length = 0
  FakeSocket.opened = []
  vi.stubGlobal('WebSocket', FakeSocket)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('a VM console', () => {
  it('connects noVNC to the console socket, scaled to the window', async () => {
    await mountApp(VmConsole, { props: { vm: '0a1b2c3d4e5f' } })
    await settle()
    expect(FakeSocket.opened).toHaveLength(1)
    expect(FakeSocket.opened[0]!.url).toMatch(/^ws:\/\/[^/]+\/api\/v1\/vms\/0a1b2c3d4e5f\/console$/)
    expect(FakeSocket.opened[0]!.protocols).toEqual(['binary'])
    const client = clients[0]!
    expect(client.channel).toBe(FakeSocket.opened[0])
    expect(client.scaleViewport && !client.resizeSession).toBe(true)
    expect(page()).toContain(t('vms.consoleWindow.connecting'))
    expect(button(t('vms.consoleWindow.ctrlAltDel')).disabled).toBe(true)

    client.dispatchEvent(new CustomEvent('connect'))
    await settle()
    await click(t('vms.consoleWindow.ctrlAltDel'))
    expect(client.sendCtrlAltDel).toHaveBeenCalledOnce()
  })

  it('says why the console closed and connects again', async () => {
    await mountApp(VmConsole, { props: { vm: '0a1b2c3d4e5f' } })
    await settle()
    FakeSocket.opened[0]!.close(4000, 'the virtual machine is not running')
    clients[0]!.dispatchEvent(new CustomEvent('disconnect'))
    await settle()
    expect(page()).toContain('the virtual machine is not running')
    await click(t('vms.consoleWindow.reconnect'))
    await settle()
    expect(FakeSocket.opened).toHaveLength(2)
  })

  it('says only that it closed when the VM stopped', async () => {
    await mountApp(VmConsole, { props: { vm: '0a1b2c3d4e5f' } })
    await settle()
    FakeSocket.opened[0]!.close(1000, 'ended')
    clients[0]!.dispatchEvent(new CustomEvent('disconnect'))
    await settle()
    expect(page()).toContain(t('vms.consoleWindow.closed'))
  })
})
