// Random passwords for new accounts: long, from characters that cannot be
// mistaken for each other when read out, with every kind a policy can ask
// for.

const LOWER = 'abcdefghijkmnpqrstuvwxyz'
const UPPER = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
const DIGITS = '23456789'
const SYMBOLS = '-_.+=!?#'
const ALL = LOWER + UPPER + DIGITS + SYMBOLS

function pick(alphabet: string, random: number): string {
  return alphabet[random % alphabet.length]!
}

export function generatePassword(length = 16): string {
  const random = new Uint32Array(length + length)
  crypto.getRandomValues(random)
  const chars = [
    pick(LOWER, random[0]!),
    pick(UPPER, random[1]!),
    pick(DIGITS, random[2]!),
    pick(SYMBOLS, random[3]!),
  ]
  for (let i = chars.length; i < length; i++) chars.push(pick(ALL, random[i]!))
  // Shuffled, so the kinds are not always in front.
  for (let i = chars.length - 1; i > 0; i--) {
    const j = random[length + i]! % (i + 1)
    ;[chars[i], chars[j]] = [chars[j]!, chars[i]!]
  }
  return chars.join('')
}
