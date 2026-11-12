/**
 * The machine's name as the server takes it: one label of letters, digits
 * and `-` (neither first nor last), at most `max` characters (15, NetBIOS's
 * limit), not digits only, not `localhost`; spaces around it and capitals
 * do not count.
 */
export type HostNameProblem = 'empty' | 'characters' | 'tooLong' | 'hyphen' | 'digits' | 'reserved'

export function tidyHostName(input: string): string {
  return input.trim().toLowerCase()
}

/** What is wrong with `input` as a name, as the server would say; `null` if nothing. */
export function hostNameProblem(input: string, max: number): HostNameProblem | null {
  const name = tidyHostName(input)
  if (!name) return 'empty'
  if (!/^[a-z0-9-]+$/.test(name)) return 'characters'
  if (name.length > max) return 'tooLong'
  if (name.startsWith('-') || name.endsWith('-')) return 'hyphen'
  if (/^[0-9]+$/.test(name)) return 'digits'
  if (name === 'localhost') return 'reserved'
  return null
}

/**
 * Where the interface is reached once the machine is called `renamed`, if
 * the browser reached it by its old name (`old`, `old.local`,
 * `old.home.arpa`): the same address with the new name. By an IP address or
 * another name it stays where it is: `null`.
 */
export function addressAfterRename(
  location: Pick<Location, 'protocol' | 'hostname' | 'port' | 'pathname'>,
  old: string,
  renamed: string,
): string | null {
  const host = location.hostname.toLowerCase()
  const previous = old.toLowerCase()
  if (!previous || previous === renamed) return null
  let next: string | null = null
  if (host === previous) next = renamed
  else if (host.startsWith(`${previous}.`)) next = renamed + host.slice(previous.length)
  if (!next) return null
  const port = location.port ? `:${location.port}` : ''
  return `${location.protocol}//${next}${port}${location.pathname}`
}
