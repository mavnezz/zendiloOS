import type { UsageMoment } from '@/shared/api/types'
import type { ChartSeries } from '@/shared/ui/chart'

export type Metric = 'cpu' | 'memory' | 'received' | 'sent'

export const COLORS = [
  'var(--chart-1)',
  'var(--chart-2)',
  'var(--chart-3)',
  'var(--chart-4)',
  'var(--chart-5)',
]

/** Every container in the samples, by name. */
export function containersIn(moments: UsageMoment[]): string[] {
  const names = new Set<string>()
  for (const moment of moments) for (const point of moment.points) names.add(point.container)
  return [...names].sort()
}

/**
 * One series per container for `metric`, a value per sample; a sample
 * without the container counts as 0. At most `limit` series, those that
 * used the most first.
 */
export function usageSeries(
  moments: UsageMoment[],
  metric: Metric,
  limit = COLORS.length,
): ChartSeries[] {
  const value = (moment: UsageMoment, name: string) => {
    const point = moment.points.find((p) => p.container === name)
    if (!point) return 0
    if (metric === 'cpu') return point.cpu_percent
    if (metric === 'memory') return point.memory_bytes
    return metric === 'received' ? point.received_per_second : point.sent_per_second
  }
  const series = containersIn(moments).map((name) => {
    const values = moments.map((moment) => value(moment, name))
    return { name, values, total: values.reduce((sum, v) => sum + v, 0) }
  })
  return series
    .sort((a, b) => b.total - a.total || a.name.localeCompare(b.name))
    .slice(0, limit)
    .map((entry, index) => ({
      label: entry.name,
      values: entry.values,
      color: COLORS[index % COLORS.length]!,
    }))
}

/** `postgres:17` → name and tag; a digest or no tag says `latest`. */
export function imageParts(image: string): { name: string; tag: string } {
  const plain = image.split('@')[0] ?? image
  const colon = plain.lastIndexOf(':')
  if (colon > plain.lastIndexOf('/'))
    return { name: plain.slice(0, colon), tag: plain.slice(colon + 1) }
  return { name: plain, tag: 'latest' }
}
