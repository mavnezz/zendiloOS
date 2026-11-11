// Opens a virtual machine's console over the API's WebSocket, as noVNC
// does, for scripts/vm-vms-test.sh: waits for QEMU's VNC server to greet
// (`RFB 003.00x`), answers with the same version, and prints the greeting
// and the security types offered. Exits 0 once both came, 1 otherwise,
// printing why the socket closed if it did.
//
//   node scripts/vm-vnc-probe.mjs <wss url> <cookie> <origin>
//
// The VM's certificate is its own: run with NODE_TLS_REJECT_UNAUTHORIZED=0.
const [url, cookie, origin] = process.argv.slice(2)
if (!url || !cookie || !origin) {
  console.error('usage: vm-vnc-probe.mjs <wss url> <cookie> <origin>')
  process.exit(2)
}

const socket = new WebSocket(url, { protocols: ['binary'], headers: { Cookie: cookie, Origin: origin } })
socket.binaryType = 'arraybuffer'
let received = new Uint8Array(0)
let greeting = null
const done = (code, message) => {
  console.log(message)
  try {
    socket.close()
  } catch {
    // Closed already.
  }
  process.exit(code)
}
const timer = setTimeout(() => done(1, `no answer in 20 s: ${greeting ?? 'no greeting'}`), 20_000)

socket.addEventListener('message', (event) => {
  if (typeof event.data === 'string') return done(1, `a text frame, which RFB never sends: ${event.data}`)
  const chunk = new Uint8Array(event.data)
  const joined = new Uint8Array(received.length + chunk.length)
  joined.set(received)
  joined.set(chunk, received.length)
  received = joined
  if (greeting === null && received.length >= 12) {
    greeting = new TextDecoder().decode(received.slice(0, 12))
    if (!/^RFB 003\.00\d\n$/.test(greeting)) return done(1, `not a VNC greeting: ${JSON.stringify(greeting)}`)
    received = received.slice(12)
    socket.send(new TextEncoder().encode(greeting))
  }
  // RFB 3.7 and 3.8: the number of security types, then the types.
  if (greeting !== null && received.length >= 1 && received.length >= 1 + received[0]) {
    clearTimeout(timer)
    const types = [...received.slice(1, 1 + received[0])]
    done(types.length > 0 ? 0 : 1, `${greeting.trim()} security types ${JSON.stringify(types)}`)
  }
})
socket.addEventListener('close', (event) =>
  done(1, `closed ${event.code} ${event.reason || ''} after ${greeting ?? 'no greeting'}`),
)
socket.addEventListener('error', () => done(1, 'the socket failed'))
