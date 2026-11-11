// Subscribes to the live WebSocket's topic `vm_stats` as the browser does,
// for scripts/vm-vms-test.sh, and waits for a sample in which the VM given
// has a CPU share (which takes two looks). Prints that sample as JSON and
// exits 0; exits 1 after 45 s without one, printing what came.
//
//   node scripts/vm-stats-probe.mjs <wss url> <cookie> <origin> <vm id>
//
// The VM's certificate is its own: run with NODE_TLS_REJECT_UNAUTHORIZED=0.
const [url, cookie, origin, id] = process.argv.slice(2)
if (!url || !cookie || !origin || !id) {
  console.error('usage: vm-stats-probe.mjs <wss url> <cookie> <origin> <vm id>')
  process.exit(2)
}

const socket = new WebSocket(url, { headers: { Cookie: cookie, Origin: origin } })
let last = 'nothing'
const done = (code, message) => {
  console.log(message)
  try {
    socket.close()
  } catch {
    // Closed already.
  }
  process.exit(code)
}
const timer = setTimeout(() => done(1, `no load of ${id} in 45 s; last: ${last}`), 45_000)

socket.addEventListener('open', () => {
  socket.send(JSON.stringify({ type: 'subscribe', topic: 'vm_stats' }))
})
socket.addEventListener('message', (event) => {
  if (typeof event.data !== 'string') return
  last = event.data
  let message
  try {
    message = JSON.parse(event.data)
  } catch {
    return
  }
  if (message.type !== 'vm_stats') return
  const load = message.data.vms.find((vm) => vm.id === id)
  if (load && typeof load.cpu_percent === 'number') {
    clearTimeout(timer)
    done(0, JSON.stringify(message.data))
  }
})
socket.addEventListener('close', (event) => {
  clearTimeout(timer)
  done(1, `closed: ${event.code} ${event.reason}; last: ${last}`)
})
socket.addEventListener('error', () => done(1, `the socket failed; last: ${last}`))
