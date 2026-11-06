import type { Share, ShareRuleLevel } from '@/shared/api/types'

export type Effective = 'read' | 'read_write' | null

/**
 * What an account may do in a share, as the server decides it: a denial
 * wins, the account's own or a group's; then the account's own rule; then
 * the broadest grant of its groups.
 */
export function effectiveAccess(
  share: Pick<Share, 'access'>,
  account: number | null,
  groups: readonly number[],
  /** The account's own rule as it is being edited; `undefined`: the stored one. */
  own?: ShareRuleLevel | null,
): Effective {
  const isOwn = (rule: Share['access'][number]) => rule.kind === 'account' && rule.id === account
  const ownRule = own !== undefined ? own : (share.access.find(isOwn)?.access ?? null)
  const groupRules = share.access.filter(
    (rule) => rule.kind === 'group' && groups.includes(rule.id),
  )
  if (ownRule === 'deny' || groupRules.some((rule) => rule.access === 'deny')) return null
  if (ownRule) return ownRule
  if (groupRules.some((rule) => rule.access === 'read_write')) return 'read_write'
  if (groupRules.some((rule) => rule.access === 'read')) return 'read'
  return null
}

/** The rule `kind` `id` has in the share, if any. */
export function ruleOf(
  share: Pick<Share, 'access'>,
  kind: 'account' | 'group',
  id: number | null,
): ShareRuleLevel | null {
  return share.access.find((rule) => rule.kind === kind && rule.id === id)?.access ?? null
}
