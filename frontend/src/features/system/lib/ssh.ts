import type { RootLoginMode, SshRequest, SshState } from '@/shared/api/types'

/** Ports the NAS's own services use: NFS, WS-Discovery, Avahi. */
const TAKEN = [2049, 3702, 5353]

/** How long SSH may stay on, in minutes. */
export const OFF_AFTER = [10, 30, 60, 240] as const

/** `system`: as Debian's configuration has it. */
export type PasswordLogin = 'system' | 'yes' | 'no'

export interface SshDraft {
  enabled: boolean
  port: string
  root_login: RootLoginMode | 'system'
  password_login: PasswordLogin
  /** `never`, `planned` (the timer that runs stays), or minutes. */
  off_after: string
}

export function toDraft(state: SshState): SshDraft {
  return {
    enabled: state.enabled,
    port: String(state.port),
    root_login: state.root_login ?? 'system',
    password_login: state.password_login == null ? 'system' : state.password_login ? 'yes' : 'no',
    off_after: state.enabled && state.off_at != null ? 'planned' : 'never',
  }
}

/** 22, or a port from 1024 that no service of the NAS uses, as the server checks it. */
export function portValid(port: string): boolean {
  if (!/^\d{1,5}$/.test(port.trim())) return false
  const value = Number(port)
  return value === 22 || (value >= 1024 && value <= 65_535 && !TAKEN.includes(value))
}

/**
 * The request for `draft`. A planned turn-off keeps its time, to the
 * minute: `offAt` and `now` in Unix seconds.
 */
export function fromDraft(
  draft: SshDraft,
  offAt: number | null | undefined,
  now: number,
): SshRequest {
  let minutes: number | null = null
  if (draft.enabled && draft.off_after === 'planned' && offAt != null) {
    minutes = Math.min(1440, Math.max(1, Math.ceil((offAt - now) / 60)))
  } else if (draft.enabled && /^\d+$/.test(draft.off_after)) {
    minutes = Number(draft.off_after)
  }
  return {
    enabled: draft.enabled,
    port: Number(draft.port.trim()),
    root_login: draft.root_login === 'system' ? null : draft.root_login,
    password_login: draft.password_login === 'system' ? null : draft.password_login === 'yes',
    off_after_minutes: minutes,
  }
}

export function sameSsh(a: SshDraft, b: SshDraft): boolean {
  return (
    a.enabled === b.enabled &&
    a.port.trim() === b.port.trim() &&
    a.root_login === b.root_login &&
    a.password_login === b.password_login &&
    a.off_after === b.off_after
  )
}
