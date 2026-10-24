// Bridge between the app (src/analyzer/web.rs) and wgsl-analyzer-web, the
// wgsl-analyzer language server compiled to wasm. Trunk copies the package to
// `wgsl-analyzer/` next to index.html (see web/fetch-wgsl-analyzer.sh).
//
// The Rust side polls instead of taking callbacks: it starts the server once,
// then sends and drains JSON-RPC messages as strings every frame.

let server = null
let state = 'idle' // idle | loading | ready | failed: <why>
let generation = 0 // bumped by stop(), so a late start() result is discarded
const inbox = []

export function analyzer_start(root, filesJson) {
  if (state !== 'idle') return
  if (!globalThis.crossOriginIsolated) {
    state =
      'failed: the page is not cross-origin isolated (no COOP/COEP headers), ' +
      'which the server needs for SharedArrayBuffer'
    return
  }
  state = 'loading'
  const mine = ++generation
  // Resolved against the page, so it works under any --public-url.
  const url = new URL('wgsl-analyzer/index.js', document.baseURI).href
  import(url)
    .then(({ WgslAnalyzerServer }) =>
      WgslAnalyzerServer.start({
        root,
        files: JSON.parse(filesJson),
        onStderr: (line) => console.debug('[wgsl-analyzer]', line),
        onExit: (code) => {
          if (mine === generation) state = `failed: wgsl-analyzer exited with code ${code}`
        },
      }),
    )
    .then((started) => {
      if (mine !== generation) {
        started.dispose()
        return
      }
      server = started
      server.onMessage((message) => inbox.push(JSON.stringify(message)))
      state = 'ready'
    })
    .catch((error) => {
      if (mine === generation) state = `failed: ${error?.message ?? error}`
    })
}

export function analyzer_stop() {
  generation++
  server?.dispose()
  server = null
  inbox.length = 0
  state = 'idle'
}

export function analyzer_state() {
  return state
}

export function analyzer_send(json) {
  server?.sendMessage(JSON.parse(json))
}

export function analyzer_drain() {
  return inbox.splice(0)
}
