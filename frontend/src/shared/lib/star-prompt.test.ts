import { describe, expect, it } from 'vitest'

import { type StarPromptFacts, starPromptVerdict } from './star-prompt'

const DAY = 86_400
const CREATED = 1_700_000_000

/** An administrator of 20 days at a calm moment, never asked. */
function facts(change: Partial<StarPromptFacts> = {}): StarPromptFacts {
  return {
    now: CREATED + 20 * DAY,
    role: 'admin',
    createdAt: CREATED,
    loaded: true,
    prompt: null,
    shownThisSession: false,
    runningJobs: 0,
    dialogOpen: false,
    alerts: 0,
    storage: null,
    ...change,
  }
}

describe('the reminder to star zendiloOS', () => {
  it('asks administrators whose account is two weeks old', () => {
    expect(starPromptVerdict(facts())).toBe('show')
    expect(starPromptVerdict(facts({ now: CREATED + 14 * DAY }))).toBe('show')
    expect(starPromptVerdict(facts({ now: CREATED + 14 * DAY - 1 }))).toBe('never')
  })

  it('never asks anyone else', () => {
    expect(starPromptVerdict(facts({ role: 'user' }))).toBe('never')
    expect(starPromptVerdict(facts({ role: 'readonly' }))).toBe('never')
    expect(starPromptVerdict(facts({ role: undefined }))).toBe('never')
    expect(starPromptVerdict(facts({ createdAt: undefined }))).toBe('never')
  })

  it('asks at most once per browser session', () => {
    expect(starPromptVerdict(facts({ shownThisSession: true }))).toBe('never')
  })

  it('waits for the account’s answer to be known', () => {
    expect(starPromptVerdict(facts({ loaded: false }))).toBe('wait')
  })

  it('asks once more two months after “Later”, and never a third time', () => {
    const later = { times_shown: 1, last_shown_at: CREATED + 20 * DAY, answer: 'later' as const }
    const at = (days: number) => CREATED + 20 * DAY + days * DAY
    expect(starPromptVerdict(facts({ prompt: later, now: at(59) }))).toBe('never')
    expect(starPromptVerdict(facts({ prompt: later, now: at(60) }))).toBe('show')
    const unanswered = { ...later, answer: null }
    expect(starPromptVerdict(facts({ prompt: unanswered, now: at(60) }))).toBe('show')
    const again = { ...later, times_shown: 2 }
    expect(starPromptVerdict(facts({ prompt: again, now: at(400) }))).toBe('never')
  })

  it('does not ask again after the link or “No thanks”', () => {
    for (const answer of ['starred', 'declined'] as const) {
      const prompt = { times_shown: 1, last_shown_at: CREATED, answer }
      expect(starPromptVerdict(facts({ prompt, now: CREATED + 400 * DAY }))).toBe('never')
    }
  })

  it('waits while a job runs, a dialog is open or something is wrong', () => {
    expect(starPromptVerdict(facts({ runningJobs: 1 }))).toBe('wait')
    expect(starPromptVerdict(facts({ dialogOpen: true }))).toBe('wait')
    expect(starPromptVerdict(facts({ alerts: 2 }))).toBe('wait')
    expect(starPromptVerdict(facts({ storage: 'warning' }))).toBe('wait')
    expect(starPromptVerdict(facts({ storage: 'critical' }))).toBe('wait')
    expect(starPromptVerdict(facts({ storage: undefined })), 'not known yet').toBe('wait')
  })

  it('takes a storage notice for no problem', () => {
    expect(starPromptVerdict(facts({ storage: 'notice' }))).toBe('show')
  })
})
