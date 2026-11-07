// Opens a container's terminal over the API's WebSocket, as the browser
// does, for scripts/vm-docker-test.sh: types a command, changes the size,
// and prints what came back. Exits 0 once both answers came, 1 otherwise.
//
//   node scripts/vm-terminal-probe.mjs <wss url> <cookie> <origin>
//
// The VM's certificate is its own: run with NODE_TLS_REJECT_UNAUTHORIZED=0.
const [url, cookie, origin] = process.argv.slice(2)
if (!url || !cookie || !origin) {
  console.error('usage: vm-terminal-probe.mjs <wss url> <cookie> <origin>')
  process.exit(2)
}

const socket = new WebSocket(url, { headers: { Cookie: cookie, Origin: origin } })
socket.binaryType = 'arraybuffer'
let seen = ''
const done = (code) => {
  console.log(seen)
  socket.close()
  process.exit(code)
}
const timer = setTimeout(() => done(1), 20_000)

socket.addEventListener('open', () => {
  socket.send(new TextEncoder().encode('echo zdl-$((6*7))\n'))
  socket.send(JSON.stringify({ type: 'resize', columns: 132, rows: 50 }))
  setTimeout(() => socket.send(new TextEncoder().encode('stty size\n')), 500)
})
socket.addEventListener('message', (event) => {
  seen += typeof event.data === 'string' ? event.data : new TextDecoder().decode(event.data)
  if (seen.includes('zdl-42') && seen.includes('50 132')) {
    clearTimeout(timer)
    done(0)
  }
})
socket.addEventListener('error', () => done(1))
socket.addEventListener('close', () => done(seen.includes('zdl-42') && seen.includes('50 132') ? 0 : 1))
