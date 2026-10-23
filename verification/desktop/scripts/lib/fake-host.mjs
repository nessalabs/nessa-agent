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
 * Every window may ask for setup again (`replay_onboarding`, from Settings),
 * which is counted in `window.__fakeHostReplays`. Given `setup`, the page is
 * the setup window the real host opens for that ask: the host says it started
 * (`host_startup`), takes the reveal, and keeps each handoff's arguments in
 * `window.__fakeHostSetupFinishes`, answering as a handoff that closed setup.
 */
export function gatewayHost({
  endpoint,
  credential,
  notices = false,
  starting = null,
  setup = false,
}) {
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
      if (command === "replay_onboarding") {
        window.__fakeHostReplays++
        return Promise.resolve(null)
      }
      if (setup && command === "host_startup") return Promise.resolve({ state: "ready" })
      if (setup && command === "reveal_setup_window") return Promise.resolve(null)
      if (setup && command === "finish_setup") {
        window.__fakeHostSetupFinishes.push(args)
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
