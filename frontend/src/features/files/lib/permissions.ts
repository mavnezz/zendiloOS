/** A rule for a folder on a volume, or none. */
export type Level = 'none' | 'deny' | 'read' | 'read_write'

/** What holds for an account: what it may do, or nothing. */
export type Effective = 'deny' | 'read' | 'read_write' | null

/**
 * What holds for an account, as the NAS decides it (sharing's `effective`):
 * a denial wins, its own or a group's; then its own rule; then the
 * broadest rule of its groups.
 */
export function effectiveLevel(own: Level | undefined, groups: (Level | undefined)[]): Effective {
  const given = groups.filter(
    (level): level is Exclude<Level, 'none'> => !!level && level !== 'none',
  )
  if (own === 'deny' || given.includes('deny')) return 'deny'
  if (own === 'read' || own === 'read_write') return own
  if (given.includes('read_write')) return 'read_write'
  if (given.includes('read')) return 'read'
  return null
}

const LABEL = /^(?!-)[a-z0-9*?-]{1,63}(?<!-)$/

/** An IP address or CIDR network, roughly; the NAS checks it exactly. */
function isAddress(value: string): boolean {
  const [address = '', prefix] = value.split('/')
  const ipv4 = /^(\d{1,3})(\.\d{1,3}){3}$/.test(address)
  const ipv6 = address.includes(':') && /^[0-9a-f:.]+$/i.test(address)
  if (!ipv4 && !ipv6) return false
  if (ipv4 && address.split('.').some((part) => Number(part) > 255)) return false
  if (prefix === undefined) return true
  const bits = Number(prefix)
  return /^\d{1,3}$/.test(prefix) && bits <= (ipv4 ? 32 : 128)
}

/**
 * Who an NFS rule may be for, as the NAS takes it: an IP address or a
 * network (`192.168.1.0/24`), a host name (`nas.local`) or a pattern with
 * `*` and `?` (`*.example.com`).
 */
export function isNfsClient(value: string): boolean {
  const client = value.trim().toLowerCase()
  if (client === '' || client.length > 253) return false
  if (isAddress(client)) return true
  return /[a-z*?]/.test(client) && client.split('.').every((label) => LABEL.test(label))
}
