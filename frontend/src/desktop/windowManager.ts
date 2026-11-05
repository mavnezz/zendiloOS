// The windows of the desktop as plain data and pure functions: every rule
// (stacking, placing, keeping title bars reachable) is testable without a
// browser. One window per app.

export interface Size {
  width: number
  height: number
}

export interface Geometry extends Size {
  x: number
  y: number
}

export interface WindowState {
  appId: string
  geometry: Geometry
  minimized: boolean
  maximized: boolean
  /** Stacking order: higher is in front. */
  z: number
}

export interface DesktopState {
  windows: WindowState[]
  nextZ: number
}

/** Height of a window's title bar, which must stay reachable. */
export const TITLE_BAR = 40
/** Width of a window that must stay on screen when moved aside. */
export const MIN_VISIBLE = 96

const CASCADE = 32
const ORIGIN = { x: 48, y: 32 }

export const emptyDesktop: DesktopState = { windows: [], nextZ: 1 }

function update(
  state: DesktopState,
  appId: string,
  change: (window: WindowState) => WindowState,
): DesktopState {
  return {
    ...state,
    windows: state.windows.map((window) => (window.appId === appId ? change(window) : window)),
  }
}

/** Keeps the title bar inside `viewport`; the window may stick out sideways. */
export function clampGeometry(geometry: Geometry, viewport: Size): Geometry {
  const width = Math.min(geometry.width, viewport.width)
  const height = Math.min(geometry.height, viewport.height)
  const minX = MIN_VISIBLE - width
  const maxX = viewport.width - MIN_VISIBLE
  const maxY = Math.max(viewport.height - TITLE_BAR, 0)
  return {
    width,
    height,
    x: Math.round(Math.min(Math.max(geometry.x, minX), maxX)),
    y: Math.round(Math.min(Math.max(geometry.y, 0), maxY)),
  }
}

export function focusWindow(state: DesktopState, appId: string): DesktopState {
  if (!state.windows.some((window) => window.appId === appId)) return state
  return {
    ...update(state, appId, (window) => ({ ...window, minimized: false, z: state.nextZ })),
    nextZ: state.nextZ + 1,
  }
}

/** Opens `appId`, or brings it to the front if it is open already. */
export function openWindow(
  state: DesktopState,
  appId: string,
  size: Size,
  viewport: Size,
): DesktopState {
  if (state.windows.some((window) => window.appId === appId)) return focusWindow(state, appId)
  const step = (state.windows.length % 8) * CASCADE
  const geometry = clampGeometry({ x: ORIGIN.x + step, y: ORIGIN.y + step, ...size }, viewport)
  // Placed so the whole window fits, if it can.
  geometry.x = Math.max(Math.min(geometry.x, viewport.width - geometry.width), 0)
  geometry.y = Math.max(Math.min(geometry.y, viewport.height - geometry.height), 0)
  const window: WindowState = {
    appId,
    geometry,
    minimized: false,
    maximized: false,
    z: state.nextZ,
  }
  return { windows: [...state.windows, window], nextZ: state.nextZ + 1 }
}

export function closeWindow(state: DesktopState, appId: string): DesktopState {
  return { ...state, windows: state.windows.filter((window) => window.appId !== appId) }
}

export function minimizeWindow(state: DesktopState, appId: string): DesktopState {
  return update(state, appId, (window) => ({ ...window, minimized: true }))
}

export function toggleMaximized(state: DesktopState, appId: string): DesktopState {
  return focusWindow(
    update(state, appId, (window) => ({ ...window, maximized: !window.maximized })),
    appId,
  )
}

export function moveWindow(
  state: DesktopState,
  appId: string,
  position: { x: number; y: number },
  viewport: Size,
): DesktopState {
  return update(state, appId, (window) =>
    window.maximized
      ? window
      : { ...window, geometry: clampGeometry({ ...window.geometry, ...position }, viewport) },
  )
}

export function resizeWindow(
  state: DesktopState,
  appId: string,
  size: Size,
  minimum: Size,
  viewport: Size,
): DesktopState {
  return update(state, appId, (window) => {
    if (window.maximized) return window
    const width = Math.min(Math.max(size.width, minimum.width), viewport.width - window.geometry.x)
    const height = Math.min(
      Math.max(size.height, minimum.height),
      viewport.height - window.geometry.y,
    )
    return { ...window, geometry: { ...window.geometry, width, height } }
  })
}

/** After the screen changed size: every title bar reachable again. */
export function fitToViewport(state: DesktopState, viewport: Size): DesktopState {
  return {
    ...state,
    windows: state.windows.map((window) => ({
      ...window,
      geometry: clampGeometry(window.geometry, viewport),
    })),
  }
}

/** The window in front, if any is visible. */
export function activeWindow(state: DesktopState): string | null {
  const visible = state.windows.filter((window) => !window.minimized)
  if (visible.length === 0) return null
  return visible.reduce((front, window) => (window.z > front.z ? window : front)).appId
}

/** Reads a stored desktop back; anything malformed or unknown is dropped. */
export function restoreDesktop(stored: unknown, knownApps: Set<string>): DesktopState {
  if (typeof stored !== 'object' || stored === null || !('windows' in stored)) return emptyDesktop
  const windows = stored.windows
  if (!Array.isArray(windows)) return emptyDesktop
  const isNumber = (value: unknown): value is number =>
    typeof value === 'number' && Number.isFinite(value)
  const valid = windows.filter(
    (window): window is WindowState =>
      typeof window === 'object' &&
      window !== null &&
      typeof window.appId === 'string' &&
      knownApps.has(window.appId) &&
      typeof window.minimized === 'boolean' &&
      typeof window.maximized === 'boolean' &&
      isNumber(window.z) &&
      typeof window.geometry === 'object' &&
      window.geometry !== null &&
      ['x', 'y', 'width', 'height'].every((key) => isNumber(window.geometry[key])),
  )
  const nextZ = Math.max(0, ...valid.map((window) => window.z)) + 1
  return { windows: valid, nextZ }
}
