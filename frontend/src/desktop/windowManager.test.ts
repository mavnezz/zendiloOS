import { describe, expect, it } from 'vitest'

import {
  activeWindow,
  closeWindow,
  emptyDesktop,
  focusWindow,
  MIN_VISIBLE,
  minimizeAll,
  minimizeWindow,
  moveWindow,
  navigateWindow,
  openWindow,
  resizeWindow,
  restoreDesktop,
  shownGeometry,
  TITLE_BAR,
  toggleMaximized,
} from './windowManager'

const screen = { width: 1280, height: 760 }
const size = { width: 640, height: 480 }

function withWindows(...appIds: string[]) {
  return appIds.reduce((state, id) => openWindow(state, id, size, screen), emptyDesktop)
}

function find(state: ReturnType<typeof withWindows>, appId: string) {
  return state.windows.find((window) => window.appId === appId)!
}

describe('the window manager', () => {
  it('opens an app on a page and keeps the page with its window', () => {
    let state = openWindow(emptyDesktop, 'settings', size, screen, 'account')
    expect(state.windows[0]?.section).toBe('account')
    state = navigateWindow(state, 'settings', 'network')
    expect(state.windows[0]?.section).toBe('network')
    state = openWindow(state, 'settings', size, screen, 'device')
    expect(state.windows).toHaveLength(1)
    expect(state.windows[0]?.section).toBe('device')
    expect(
      restoreDesktop(JSON.parse(JSON.stringify(state)), new Set(['settings'])).windows[0]?.section,
    ).toBe('device')
  })

  it('shows the desktop by putting every window away', () => {
    let state = openWindow(emptyDesktop, 'files', size, screen)
    state = openWindow(state, 'logs', size, screen)
    expect(minimizeAll(state).windows.every((window) => window.minimized)).toBe(true)
  })

  it('opens apps cascaded, each in front of the previous', () => {
    const state = withWindows('dashboard', 'settings')
    const [first, second] = state.windows
    expect(second!.geometry.x - first!.geometry.x).toBe(32)
    expect(second!.z).toBeGreaterThan(first!.z)
    expect(activeWindow(state)).toBe('settings')
  })

  it('keeps one window per app and brings it back to the front', () => {
    const state = openWindow(
      minimizeWindow(withWindows('dashboard', 'settings'), 'dashboard'),
      'dashboard',
      size,
      screen,
    )
    expect(state.windows).toHaveLength(2)
    expect(find(state, 'dashboard').minimized).toBe(false)
    expect(activeWindow(state)).toBe('dashboard')
  })

  it('fits a window that is larger than the screen', () => {
    const state = openWindow(emptyDesktop, 'big', { width: 3000, height: 2000 }, screen)
    expect(find(state, 'big').geometry).toEqual({ x: 0, y: 0, width: 1280, height: 760 })
  })

  it('never lets a title bar leave the screen', () => {
    let state = withWindows('dashboard')
    state = moveWindow(state, 'dashboard', { x: -5000, y: -300 }, screen)
    expect(find(state, 'dashboard').geometry).toMatchObject({ x: MIN_VISIBLE - 640, y: 0 })
    state = moveWindow(state, 'dashboard', { x: 5000, y: 5000 }, screen)
    expect(find(state, 'dashboard').geometry).toMatchObject({
      x: 1280 - MIN_VISIBLE,
      y: 760 - TITLE_BAR,
    })
  })

  it('resizes within limits and leaves maximized windows alone', () => {
    let state = withWindows('dashboard')
    state = resizeWindow(
      state,
      'dashboard',
      { width: 100, height: 5000 },
      { width: 320, height: 240 },
      screen,
    )
    expect(find(state, 'dashboard').geometry).toMatchObject({ width: 320, height: 760 - 32 })

    state = toggleMaximized(state, 'dashboard')
    const before = find(state, 'dashboard').geometry
    state = moveWindow(state, 'dashboard', { x: 400, y: 400 }, screen)
    expect(find(state, 'dashboard').geometry).toEqual(before)
  })

  it('picks the next window when the front one is minimized or closed', () => {
    let state = withWindows('dashboard', 'settings', 'logs')
    state = minimizeWindow(state, 'logs')
    expect(activeWindow(state)).toBe('settings')
    state = closeWindow(state, 'settings')
    expect(activeWindow(state)).toBe('dashboard')
    state = focusWindow(state, 'logs')
    expect(activeWindow(state)).toBe('logs')
  })

  it('shows windows on a smaller screen without forgetting their size', () => {
    const state = moveWindow(withWindows('dashboard'), 'dashboard', { x: 1100, y: 700 }, screen)
    const small = { width: 600, height: 400 }

    expect(shownGeometry(find(state, 'dashboard').geometry, small)).toEqual({
      x: 600 - MIN_VISIBLE,
      y: 400 - TITLE_BAR,
      width: 600,
      height: 400,
    })
    // Back on the large screen, the window is as the user left it.
    expect(shownGeometry(find(state, 'dashboard').geometry, screen)).toEqual(
      find(state, 'dashboard').geometry,
    )
  })

  it('keeps the chosen size when a window is moved on a smaller screen', () => {
    const small = { width: 600, height: 400 }
    const state = moveWindow(withWindows('dashboard'), 'dashboard', { x: 10, y: 20 }, small)

    expect(find(state, 'dashboard').geometry).toMatchObject({
      x: 10,
      y: 20,
      width: 640,
      height: 480,
    })
  })

  it('restores a stored desktop and drops what it does not know', () => {
    const stored = JSON.parse(JSON.stringify(withWindows('dashboard', 'settings')))
    stored.windows.push({
      appId: 'removed-app',
      geometry: { x: 0, y: 0, width: 1, height: 1 },
      minimized: false,
      maximized: false,
      z: 9,
    })
    stored.windows.push({ appId: 'settings', geometry: 'broken' })

    const state = restoreDesktop(stored, new Set(['dashboard', 'settings']))

    expect(state.windows.map((window) => window.appId)).toEqual(['dashboard', 'settings'])
    expect(state.nextZ).toBe(3)
    expect(restoreDesktop('garbage', new Set())).toEqual(emptyDesktop)
  })
})
