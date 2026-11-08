/**
 * The reminder to star zendiloOS on GitHub: when the desktop shows it.
 * Nothing is asked of GitHub or anyone else, and nothing is counted or
 * sent; the button is only a link. What the account answered is kept with
 * its preferences, so every browser knows.
 */
import type { AccountRole, InterfaceStarPrompt, StorageSeverity } from '@/shared/api/types'

/** Where zendiloOS lives. */
export const PROJECT_URL = 'https://github.com/mavnezz/zendiloOS'

const DAY = 86_400
/** An account this young has not seen enough of zendiloOS to judge it. */
export const FIRST_AFTER_DAYS = 14
/** After "Later", it comes once more, this much later. */
export const AGAIN_AFTER_DAYS = 60
export const MAX_SHOWN = 2
/** How long the desktop settles after it appeared before the reminder is considered. */
export const DELAY_MS = 10_000
/** How often the desktop looks again while the moment is not calm. */
export const RETRY_MS = 5_000

/** What the desktop knows when it considers showing the reminder. */
export interface StarPromptFacts {
  /** Unix seconds. */
  now: number
  role: AccountRole | undefined
  /** When the account was created, Unix seconds. */
  createdAt: number | undefined
  /** Whether the account's preferences arrived; until then its answer is unknown. */
  loaded: boolean
  /** `null` while it was never shown. */
  prompt: InterfaceStarPrompt | null
  /** Shown in this browser session already. */
  shownThisSession: boolean
  runningJobs: number
  /** A dialog, the launcher or a menu is open. */
  dialogOpen: boolean
  /** Unread warnings and errors in the notification centre, and toasts on screen. */
  alerts: number
  /** The storage overview's verdict: `undefined` while not known. */
  storage: StorageSeverity | null | undefined
}

/**
 * `show` now; `wait` for a calm moment (or for what is not known yet);
 * `never` in this browser session.
 */
export type StarPromptVerdict = 'show' | 'wait' | 'never'

/**
 * Whether to show the reminder: to administrators only, once their account
 * is two weeks old, at most once per browser session and twice in all, the
 * second time two months after "Later" (or after it was shown and left
 * unanswered). Only at a calm moment: no job running, no dialog open, and
 * nothing wrong: no unread warning or error, no toast, and the storage
 * fine (a notice, such as a pool without redundancy, is no problem).
 */
export function starPromptVerdict(facts: StarPromptFacts): StarPromptVerdict {
  if (facts.role !== 'admin' || facts.createdAt === undefined || facts.shownThisSession) {
    return 'never'
  }
  if (!facts.loaded) return 'wait'
  if (facts.now - facts.createdAt < FIRST_AFTER_DAYS * DAY) return 'never'
  const prompt = facts.prompt
  if (prompt) {
    if (prompt.times_shown >= MAX_SHOWN) return 'never'
    if (prompt.answer === 'starred' || prompt.answer === 'declined') return 'never'
    if (facts.now - prompt.last_shown_at < AGAIN_AFTER_DAYS * DAY) return 'never'
  }
  const calm =
    facts.runningJobs === 0 &&
    !facts.dialogOpen &&
    facts.alerts === 0 &&
    (facts.storage === null || facts.storage === 'notice')
  return calm ? 'show' : 'wait'
}
