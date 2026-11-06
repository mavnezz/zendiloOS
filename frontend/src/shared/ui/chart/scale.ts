/** A round number at or above `value`: 1, 2, 2.5 or 5 times a power of `base` steps. */
export function niceMax(value: number, base: 10 | 1024 = 10): number {
  if (!(value > 0)) return 1
  let unit = 1
  if (base === 1024) {
    while (value / unit >= 1024) unit *= 1024
  }
  const scaled = value / unit
  const power = 10 ** Math.floor(Math.log10(scaled))
  const step = [1, 2, 2.5, 5, 10].find((candidate) => candidate * power >= scaled) ?? 10
  return step * power * unit
}

export interface Point {
  x: number
  y: number
}

/**
 * An SVG path through `points` as a monotone cubic curve (Fritsch–Carlson):
 * smooth, but never above the highest or below the lowest neighbouring
 * value, so a chart shows no peaks that were not measured.
 */
export function monotonePath(points: Point[]): string {
  const n = points.length
  if (n === 0) return ''
  if (n === 1) return `M${points[0]!.x},${points[0]!.y}`
  const dx: number[] = []
  const slope: number[] = []
  for (let i = 0; i < n - 1; i++) {
    dx.push(points[i + 1]!.x - points[i]!.x)
    slope.push((points[i + 1]!.y - points[i]!.y) / (dx[i] || 1))
  }
  const tangent: number[] = [slope[0]!]
  for (let i = 1; i < n - 1; i++) {
    tangent.push(slope[i - 1]! * slope[i]! <= 0 ? 0 : (slope[i - 1]! + slope[i]!) / 2)
  }
  tangent.push(slope[n - 2]!)
  for (let i = 0; i < n - 1; i++) {
    if (slope[i] === 0) {
      tangent[i] = 0
      tangent[i + 1] = 0
      continue
    }
    const a = tangent[i]! / slope[i]!
    const b = tangent[i + 1]! / slope[i]!
    const h = a * a + b * b
    if (h > 9) {
      const t = 3 / Math.sqrt(h)
      tangent[i] = t * a * slope[i]!
      tangent[i + 1] = t * b * slope[i]!
    }
  }
  let path = `M${points[0]!.x.toFixed(1)},${points[0]!.y.toFixed(1)}`
  for (let i = 0; i < n - 1; i++) {
    const p0 = points[i]!
    const p1 = points[i + 1]!
    const third = dx[i]! / 3
    const c1 = `${(p0.x + third).toFixed(1)},${(p0.y + tangent[i]! * third).toFixed(1)}`
    const c2 = `${(p1.x - third).toFixed(1)},${(p1.y - tangent[i + 1]! * third).toFixed(1)}`
    path += ` C${c1} ${c2} ${p1.x.toFixed(1)},${p1.y.toFixed(1)}`
  }
  return path
}

/** How many equal steps divide `top` into round values: 5 for 1, 2.5 and 5, else 4. */
export function divisions(top: number): number {
  const leading = top / 10 ** Math.floor(Math.log10(top))
  const binary = top / 1024 ** Math.floor(Math.log(top) / Math.log(1024))
  const lead = Number.isInteger(binary * 100) && binary >= 1 ? binary : leading
  const digits = lead / 10 ** Math.floor(Math.log10(lead))
  return [1, 2.5, 5].some((value) => Math.abs(value - digits) < 1e-9) ? 5 : 4
}
