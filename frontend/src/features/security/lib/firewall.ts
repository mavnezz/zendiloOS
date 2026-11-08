import type {
  FirewallAction,
  FirewallChange,
  FirewallRule,
  FirewallService,
  FirewallSettings,
} from '@/shared/api/types'

/** A rule as edited, with a key of its own for the list. */
export interface RuleDraft extends FirewallRule {
  id: number
}

export interface FirewallDraft {
  enabled: boolean
  fallback: FirewallAction
  rules: RuleDraft[]
}

let lastId = 0

/** A key for a new rule in the list. */
export function ruleId(): number {
  lastId += 1
  return lastId
}

export function toDraft(settings: FirewallSettings): FirewallDraft {
  return {
    enabled: settings.enabled,
    fallback: settings.fallback,
    rules: settings.rules.map((rule) => ({ ...rule, id: ruleId() })),
  }
}

/** A rule as the server takes it: only what its service needs. */
export function plainRule(rule: FirewallRule): FirewallRule {
  return {
    enabled: rule.enabled,
    interface: rule.interface || null,
    service: rule.service,
    project: rule.service === 'project' ? (rule.project ?? null) : null,
    protocol: rule.service === 'ports' ? (rule.protocol ?? 'both') : null,
    ports: rule.service === 'ports' ? (rule.ports ?? '').trim() : null,
    source: rule.source.trim() || 'any',
    action: rule.action,
  }
}

export function fromDraft(draft: FirewallDraft): FirewallChange {
  return {
    enabled: draft.enabled,
    fallback: draft.fallback,
    rules: draft.rules.map(plainRule),
  }
}

export function sameDraft(a: FirewallDraft, b: FirewallDraft): boolean {
  return JSON.stringify(fromDraft(a)) === JSON.stringify(fromDraft(b))
}

/** `list` with the entry at `from` moved to `to`. */
export function moved<T>(list: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= list.length || to >= list.length) return list
  const copy = [...list]
  const [entry] = copy.splice(from, 1)
  copy.splice(to, 0, entry as T)
  return copy
}

/** As the server reads ports: numbers and ranges from 1 to 65535, at most 32. */
export function portsValid(text: string): boolean {
  const parts = text.split(/[\s,]+/).filter(Boolean)
  if (parts.length === 0 || parts.length > 32) return false
  return parts.every((part) => {
    const match = /^(\d{1,5})(?:-(\d{1,5}))?$/.exec(part)
    if (!match) return false
    const from = Number(match[1])
    const to = Number(match[2] ?? match[1])
    return from >= 1 && to <= 65535 && from <= to
  })
}

const IPV4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/
const IPV6 = /^[0-9a-fA-F:]+(?:\d{1,3}(?:\.\d{1,3}){3})?$/

function address(text: string): 4 | 6 | null {
  const v4 = IPV4.exec(text)
  if (v4) return v4.slice(1).every((part) => Number(part) <= 255) ? 4 : null
  return text.includes(':') && IPV6.test(text) ? 6 : null
}

/** Roughly as the server reads sources; it checks them exactly. */
export function sourceValid(text: string): boolean {
  const value = text.trim()
  if (value === '' || value.toLowerCase() === 'any') return true
  const [ip, prefix, rest] = value.split('/')
  if (prefix !== undefined) {
    const family = address(ip!.trim())
    const bits = Number(prefix)
    return (
      rest === undefined &&
      family !== null &&
      /^\d{1,3}$/.test(prefix.trim()) &&
      bits <= (family === 4 ? 32 : 128)
    )
  }
  const [from, to, more] = value.split('-')
  if (to !== undefined) {
    const kind = address(from!.trim())
    return more === undefined && kind !== null && kind === address(to.trim())
  }
  return address(value) !== null
}

/** The key the server lists a rule's service under (`smb`, `project:media`). */
export function serviceKey(rule: FirewallRule): string {
  return rule.service === 'project' ? `project:${rule.project ?? ''}` : rule.service
}

/** What a rule's service carries: TCP, UDP, both, ICMP or everything. */
export function protocolOf(
  rule: FirewallRule,
  services: FirewallService[],
): 'tcp' | 'udp' | 'both' | 'icmp' | 'all' | null {
  if (rule.service === 'all') return 'all'
  if (rule.service === 'ping') return 'icmp'
  if (rule.service === 'ports') return rule.protocol ?? 'both'
  const service = services.find((s) => s.key === serviceKey(rule))
  if (!service) return null
  if (service.tcp && service.udp) return 'both'
  if (service.tcp) return 'tcp'
  return service.udp ? 'udp' : null
}

/** A rule's ports as `139, 445 / 137-138` (TCP / UDP); empty for none. */
export function portsOf(rule: FirewallRule, services: FirewallService[]): string {
  if (rule.service === 'ports') return rule.ports ?? ''
  const service = services.find((s) => s.key === serviceKey(rule))
  if (!service) return ''
  return [service.tcp, service.udp].filter(Boolean).join(' / ')
}
