/** Browser and system from a user agent, as people name them: "Firefox · Windows". */
export function describeUserAgent(agent: string | null | undefined): string | null {
  if (!agent) return null
  const browser = [
    ['Edg/', 'Edge'],
    ['OPR/', 'Opera'],
    ['Firefox/', 'Firefox'],
    ['Chrome/', 'Chrome'],
    ['Safari/', 'Safari'],
    ['curl/', 'curl'],
  ].find(([marker]) => agent.includes(marker!))?.[1]
  const system = [
    ['Android', 'Android'],
    ['iPhone', 'iOS'],
    ['iPad', 'iPadOS'],
    ['Windows', 'Windows'],
    ['Mac OS X', 'macOS'],
    ['Linux', 'Linux'],
  ].find(([marker]) => agent.includes(marker!))?.[1]
  const parts = [browser, system].filter(Boolean)
  return parts.length ? parts.join(' · ') : agent.slice(0, 40)
}
