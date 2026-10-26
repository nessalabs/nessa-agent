/**
 * The desktop app's host, faked for a page: what `gateway-states.mjs` and
 * `gateway-window.mjs` run the real frontend against, so `host.kind` is
 * native and `main.tsx` composes `hostGateway` over IPC.
 */

import timing from "../../../../src/host/startup-timing.json" with { type: "json" }

/**
 * A host that can be held in startup: the session storage key that holds it
 * (`gatewayHost`'s `starting`), and how long it holds the desktop window's
 * read meanwhile — the real host's own number (`desktop_startup_wait` in
 * `src-tauri`), from the file both read.
 */
export const STARTING = {
  key: "nessa-fake-host-starting",
  waitMs: timing.desktopStartupWaitMs,
}

/**
 * Installed before the page's own scripts, as Tauri installs its IPC. The
 * endpoint and credential commands answer as the scenario says, and are
 * counted (the endpoint's asks timed too); every other command waits
 * forever, as an unanswered host does.
 *
 * With `notices`, the host also keeps the system's review notices as
 * `src-tauri/src/review_notices` does: notices are allowed, each
 * `review_notice_sync` set is diffed against the notices up (posted and
 * withdrawn are counted), and `window.__fakeHostOpenNotice(conversationId)`
 * is the person clicking one, sent to the window as the host's event. What
 * is up is kept in session storage, so a reloaded window must take back
 * what it no longer waits on, as it must from the real host.
 *
 * Given `starting` (`STARTING`), while its key is set in session storage the
 * host is still starting the gateway: the endpoint is answered once the key
 * is removed, as the real host holds the desktop window's read through a
 * startup still running (`GatewayReader`), and refused as not ready if it is
 * still set after `waitMs`. A scenario sets it before a reload and removes it
 * once the gateway is "up": a launch.
 *
 * A new channel's pickers answer from `window.__fakeHostPicks`: the next
 * folder chosen (`folders`, null for a cancel) and the next files or folders
 * (`resources`, an array per pick), each taken once, in order; a pick with
 * none queued is a cancel. Each pick is counted in `window.__fakeHostPicked`
 * with the kind it asked for. The home folder is `window.__fakeHostHome`.
 * Files dragged onto the window are a step's to send, as the host's
 * `nessa://paths-dragging` and `nessa://paths-dropped` (`__fakeHostEmit`).
 *
 * Every window may ask for setup again (`replay_onboarding`, from Settings),
 * which is counted in `window.__fakeHostReplays`. Given `setup`, the page is
 * the setup window the real host opens for that ask: the host says it started
 * (`host_startup`) and that its gateway is up (`gateway_startup`), takes the
 * reveal, and keeps each handoff's arguments in
 * `window.__fakeHostSetupFinishes` with whether the setup window was still
 * on screen (`windowShown`), answering as a handoff that closed setup.
 * It says there is no linger question (a host with no logind), and answers
 * where its menu bar icon is (`setup_tray_point`), 80 px in from the page's
 * top right, counting each ask in `window.__fakeHostTrayAsks`.
 *
 * Given `panel`, the page is the menu-bar panel: the host says it started and
 * its gateway is up, as for setup, takes event listeners, answers the
 * panel's size (`panel_size`) with the page's own, and says setup chose no
 * agent (`chosen_agent`).
 */
export function gatewayHost({
  endpoint,
  credential,
  notices = false,
  starting = null,
  setup = false,
  panel = false,
}) {
  // Setup and the panel both wait for the host to say it is up.
  const started = setup || panel
  // The host's events for files and folders dragged onto the window. Here,
  // not beside the function: the page runs this function's text alone.
  const DRAGGED_EVENTS = new Set(["nessa://paths-dragging", "nessa://paths-dropped"])
  // The host's typed refusals (`SurfaceCommandError`). The page maps the
  // reason; a sentence here would be an untyped failure.
  const notReady = { reason: "not-ready" }
  const notProvisioned = { reason: "not-provisioned" }
  let callbacks = 0
  const handlers = new Map()
  const asked = { load_gateway_endpoint: 0, load_surface_credential: 0 }
  window.__fakeHostAsked = asked
  // When each endpoint ask came, in the page's `performance.now()`.
  window.__fakeHostAskTimes = []
  window.__fakeHostReplays = 0
  window.__fakeHostSetupFinishes = []
  window.__fakeHostTrayAsks = 0
  window.__fakeHostPicks = { folders: [], resources: [] }
  window.__fakeHostPicked = []
  window.__fakeHostHome = "/home/person"
  // Who listens for which host event, so a step can send one as the host
  // would, once something is listening for it.
  const listening = []
  const listeners = (event) =>
    listening.filter((each) => each.event === event && handlers.has(each.id))
  window.__fakeHostListening = (event) => listeners(event).length
  window.__fakeHostEmit = (event, payload) => {
    const reached = listeners(event)
    for (const { id } of reached) handlers.get(id)({ event, id, payload })
    return reached.length
  }
  window.__TAURI_INTERNALS__ = {
    transformCallback: (callback) => {
      const id = ++callbacks
      handlers.set(id, callback)
      return id
    },
    invoke(command, args) {
      if (notices) {
        const answered = noticeHost(command, args)
        if (answered) return answered
      }
      if (command === "choose_channel_folder") {
        window.__fakeHostPicked.push("folder")
        return Promise.resolve(window.__fakeHostPicks.folders.shift() ?? null)
      }
      if (command === "choose_channel_resources") {
        window.__fakeHostPicked.push(args.folders ? "folders" : "files")
        return Promise.resolve(window.__fakeHostPicks.resources.shift() ?? [])
      }
      if (command === "home_folder") return Promise.resolve(window.__fakeHostHome)
      if (command === "replay_onboarding") {
        window.__fakeHostReplays++
        return Promise.resolve(null)
      }
      if (started && command === "host_startup")
        return Promise.resolve({ state: "ready" })
      // The gateway it manages is up: the stack's gateway is already running,
      // and nothing about it changes, so its listener is never told. Other
      // events reach a page only when a step plays the host (`__fakeHostEmit`).
      if (started && command === "gateway_startup")
        return Promise.resolve({ revision: 1, state: "ready" })
      // Every window listens for files dragged onto it, which a step sends
      // as the host does (`__fakeHostEmit`).
      const dragged = DRAGGED_EVENTS.has(args?.event)
      if ((started || dragged) && command === "plugin:event|listen") {
        window.__TAURI_EVENT_PLUGIN_INTERNALS__ ??= { unregisterListener() {} }
        listening.push({ event: args.event, id: args.handler })
        return Promise.resolve(args.handler)
      }
      if ((started || dragged) && command === "plugin:event|unlisten") {
        const at = listening.findIndex((each) => each.id === args.eventId)
        if (at >= 0) listening.splice(at, 1)
        return Promise.resolve(null)
      }
      if (panel && command === "panel_size")
        return Promise.resolve({ width: innerWidth, height: innerHeight })
      // No agent chosen in setup: a new chat starts on the gateway's default.
      if (panel && command === "chosen_agent") return Promise.resolve(null)
      if (setup && command === "reveal_setup_window") return Promise.resolve(null)
      if (setup && command === "linger_status")
        return Promise.resolve({ shown: "not-applicable" })
      if (setup && command === "setup_tray_point") {
        window.__fakeHostTrayAsks++
        return Promise.resolve({ x: innerWidth - 80, y: 12 })
      }
      if (setup && command === "finish_setup") {
        // With what was on screen as it came: a handoff waits for the window
        // to land, so the window is gone by then.
        window.__fakeHostSetupFinishes.push({
          ...args,
          windowShown: document.querySelector(".nessa-setup-window") !== null,
        })
        return Promise.resolve({ setupClosed: true, closeError: null, recordError: null })
      }
      if (command === "load_gateway_endpoint") {
        asked[command]++
        window.__fakeHostAskTimes.push(performance.now())
        if (endpoint === null) return Promise.reject(notReady)
        if (!starting) return Promise.resolve(endpoint)
        return startedBy(performance.now() + starting.waitMs).then(() => endpoint)
      }
      if (command === "load_surface_credential") {
        asked[command]++
        return credential === null
          ? Promise.reject(notProvisioned)
          : Promise.resolve(credential)
      }
      return new Promise(() => {})
    },
  }
  function stillStarting() {
    try {
      return sessionStorage.getItem(starting.key) !== null
    } catch {
      return false
    }
  }

  // Answers once startup is over, or refuses as not ready at `deadline`.
  function startedBy(deadline) {
    return new Promise((resolve, reject) => {
      const check = () => {
        if (!stillStarting()) resolve()
        else if (performance.now() >= deadline) reject(notReady)
        else setTimeout(check, 50)
      }
      check()
    })
  }

  // Up from the start, so a window that never sends a set is seen to.
  if (notices) noticeHost(null, null)

  function noticeHost(command, args) {
    const OPENED = "nessa://review-notice-opened"
    const KEPT = "nessa-fake-host-notices"
    if (!window.__fakeHostNotices) {
      let up
      try {
        up = JSON.parse(sessionStorage.getItem(KEPT) ?? "[]")
      } catch {
        up = []
      }
      window.__fakeHostNotices = { up, posted: [], withdrawn: [], syncs: 0, opened: [] }
      window.__fakeHostOpenNotice = (conversationId) => {
        const listening = window.__fakeHostNotices.opened
        for (const id of listening)
          handlers.get(id)?.({ event: OPENED, id, payload: conversationId })
        return listening.length
      }
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ ??= { unregisterListener() {} }
    }
    const state = window.__fakeHostNotices
    if (command === "review_notice_access" || command === "review_notice_ask")
      return Promise.resolve("allowed")
    if (command === "review_notice_sync") {
      const key = (each) =>
        `${each.conversationId}\u0000${each.executionId}\u0000${each.permissionId}`
      const next = args.reviews
      const staying = new Set(next.map(key))
      const shown = new Set(state.up.map(key))
      const arrived = next.filter((each) => !shown.has(key(each)))
      state.withdrawn.push(...state.up.filter((each) => !staying.has(key(each))))
      state.posted.push(...arrived)
      // A notice already up keeps the words it was posted with, as the
      // system's does: the host never posts it again.
      state.up = [
        ...state.up.filter((each) => staying.has(key(each))),
        ...arrived.map((each) => ({ ...each })),
      ]
      state.syncs++
      try {
        sessionStorage.setItem(KEPT, JSON.stringify(state.up))
      } catch {
        // Kept for this page only.
      }
      return Promise.resolve(null)
    }
    if (command === "plugin:event|listen" && args?.event === OPENED) {
      state.opened.push(args.handler)
      return Promise.resolve(args.handler)
    }
    if (command === "plugin:event|unlisten" && args?.event === OPENED) {
      state.opened = state.opened.filter((each) => each !== args.eventId)
      return Promise.resolve(null)
    }
    return null
  }
}
