/**
 * Whether every word of `query` occurs somewhere in `text`, in any order and
 * whatever the case. An empty query matches everything.
 */
export function matchesWords(text: string, query: string): boolean {
  const haystack = text.toLowerCase()
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word))
}
