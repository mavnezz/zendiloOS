// The part of noVNC's RFB client the console uses (docs/API.md of
// @novnc/novnc, which ships no types of its own).
declare module '@novnc/novnc' {
  export interface RfbOptions {
    shared?: boolean
    wsProtocols?: string[]
  }

  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, urlOrChannel: string | WebSocket, options?: RfbOptions)
    /** Scales the remote screen to the element, keeping its proportions. */
    scaleViewport: boolean
    /** Asks the server to change its size to the element's; QEMU does not. */
    resizeSession: boolean
    viewOnly: boolean
    focusOnClick: boolean
    background: string
    disconnect(): void
    sendCtrlAltDel(): void
    focus(): void
  }
}
