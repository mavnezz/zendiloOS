/** A line of a project's log: which container wrote it, and what. */
export interface LogLine {
  container: string | null
  text: string
}

/** `teslamate-grafana-1  | logger=…`: the container, padded, a bar, the line. */
const PREFIXED = /^(\S+)\s+\| ?(.*)$/

/**
 * What `docker compose logs` printed, line by line: each under the
 * container before its bar; a line without one (wrapped, or Compose's own)
 * stays under the container before it. Empty lines go.
 */
export function parseProjectLog(text: string): LogLine[] {
  const lines: LogLine[] = []
  let container: string | null = null
  for (const raw of text.split('\n')) {
    if (!raw.trim()) continue
    const match = PREFIXED.exec(raw)
    if (match) {
      container = match[1]!
      lines.push({ container, text: match[2]! })
    } else {
      lines.push({ container, text: raw })
    }
  }
  return lines
}

/** Text colours readable on the log's dark background, one per container. */
export const LOG_COLORS = [
  'text-sky-300',
  'text-emerald-300',
  'text-amber-300',
  'text-fuchsia-300',
  'text-rose-300',
  'text-lime-300',
  'text-cyan-300',
  'text-orange-300',
] as const

/**
 * A colour for each container: in the order `known` lists them (the
 * project's containers), others after them as they come.
 */
export function logColors(known: string[], lines: LogLine[]): Map<string, string> {
  const colors = new Map<string, string>()
  const add = (name: string) => {
    if (!colors.has(name)) colors.set(name, LOG_COLORS[colors.size % LOG_COLORS.length]!)
  }
  for (const name of [...known].sort()) add(name)
  for (const line of lines) if (line.container) add(line.container)
  return colors
}

/** The lines with `query` in them, or under a container whose name has it, case aside. */
export function matching(lines: LogLine[], query: string): LogLine[] {
  const wanted = query.trim().toLowerCase()
  if (!wanted) return lines
  return lines.filter(
    (line) =>
      line.text.toLowerCase().includes(wanted) || line.container?.toLowerCase().includes(wanted),
  )
}
