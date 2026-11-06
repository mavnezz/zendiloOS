import type { SecuritySettings, TlsVersion } from '@/shared/api/types'

/** The settings as the page edits them: numbers as typed. */
export interface SecurityDraft {
  idle_minutes: string
  lock: boolean
  lock_failures: string
  lock_window: string
  block: boolean
  block_failures: string
  block_window: string
  /** Blocks are lifted after `release_days`; otherwise by hand. */
  release: boolean
  release_days: string
  tls: TlsVersion
}

/** Fields of the draft that the server would refuse. */
export type Invalid = Set<keyof SecurityDraft>

export function toDraft(settings: SecuritySettings, tls: TlsVersion): SecurityDraft {
  const lock = settings.account_lock
  const block = settings.auto_block
  return {
    idle_minutes: String(settings.idle_minutes),
    lock: lock != null,
    lock_failures: String(lock?.failures ?? 5),
    lock_window: String(lock?.window_minutes ?? 15),
    block: block != null,
    block_failures: String(block?.failures ?? 10),
    block_window: String(block?.window_minutes ?? 5),
    release: block == null || block.release_after_days != null,
    release_days: String(block?.release_after_days ?? 1),
    tls,
  }
}

function inRange(text: string, low: number, high: number): boolean {
  if (!/^\d+$/.test(text.trim())) return false
  const value = Number(text)
  return value >= low && value <= high
}

/** As the server checks them; only what is turned on counts. */
export function invalid(draft: SecurityDraft): Invalid {
  const wrong: Invalid = new Set()
  if (!inRange(draft.idle_minutes, 5, 1440)) wrong.add('idle_minutes')
  if (draft.lock) {
    if (!inRange(draft.lock_failures, 3, 100)) wrong.add('lock_failures')
    if (!inRange(draft.lock_window, 1, 1440)) wrong.add('lock_window')
  }
  if (draft.block) {
    if (!inRange(draft.block_failures, 3, 100)) wrong.add('block_failures')
    if (!inRange(draft.block_window, 1, 1440)) wrong.add('block_window')
    if (draft.release && !inRange(draft.release_days, 1, 365)) wrong.add('release_days')
  }
  return wrong
}

/** The settings to send; the allowed addresses are edited elsewhere. */
export function fromDraft(draft: SecurityDraft, allowed: string[]): SecuritySettings {
  const number = (text: string) => Number(text.trim())
  return {
    idle_minutes: number(draft.idle_minutes),
    account_lock: draft.lock
      ? { failures: number(draft.lock_failures), window_minutes: number(draft.lock_window) }
      : null,
    auto_block: draft.block
      ? {
          failures: number(draft.block_failures),
          window_minutes: number(draft.block_window),
          release_after_days: draft.release ? number(draft.release_days) : null,
        }
      : null,
    allowed,
  }
}

/** Whether the server would store the same, apart from the TLS version. */
export function sameSettings(a: SecurityDraft, b: SecurityDraft): boolean {
  return JSON.stringify(fromDraft(a, [])) === JSON.stringify(fromDraft(b, []))
}
