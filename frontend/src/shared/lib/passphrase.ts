/**
 * A passphrase as a header carries it: base64 of its UTF-8, so any
 * character goes (a header takes only ASCII).
 */
export function passphraseHeader(passphrase: string): string {
  let binary = ''
  for (const byte of new TextEncoder().encode(passphrase)) binary += String.fromCharCode(byte)
  return btoa(binary)
}

/** Waits until zendiloosd answers again after a restart, at most `seconds`. */
export async function waitForServer(seconds = 120, pause = 2000): Promise<boolean> {
  // It is still up just after the answer: wait for it to go first.
  await new Promise((resolve) => setTimeout(resolve, pause))
  for (let tries = 0; tries < Math.ceil((seconds * 1000) / pause); tries++) {
    try {
      const response = await fetch('/api/v1/health', { cache: 'no-store' })
      if (response.ok) return true
    } catch {
      // Not there yet.
    }
    await new Promise((resolve) => setTimeout(resolve, pause))
  }
  return false
}
