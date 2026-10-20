/**
 * The desktop app's host, faked for a page: what `gateway-states.mjs` and
 * `gateway-window.mjs` run the real frontend against, so `host.kind` is
 * native and `main.tsx` composes `hostGateway` over IPC.
 */

/**
 * Installed before the page's own scripts, as Tauri installs its IPC. The
 * endpoint and credential commands answer as the scenario says, and are
 * counted (the endpoint's asks timed too); where the page asks the window's
 * controls to stand is kept (`__fakeHostLeads`); every other command waits
 * forever, as an unanswered host does.
 */
export function gatewayHost({ endpoint, credential }) {
  // The host's typed refusals (`SurfaceCommandError`). The page maps the
  // reason; a sentence here would be an untyped failure.
  const notReady = { reason: "not-ready" }
  const notProvisioned = { reason: "not-provisioned" }
  let callbacks = 0
  const asked = { load_gateway_endpoint: 0, load_surface_credential: 0 }
  window.__fakeHostAsked = asked
  // When each endpoint ask came, in the page's `performance.now()`.
  window.__fakeHostAskTimes = []
  window.__fakeHostLeads = []
  window.__TAURI_INTERNALS__ = {
    transformCallback: () => ++callbacks,
    invoke(command, args) {
      if (command === "load_gateway_endpoint") {
        asked[command]++
        window.__fakeHostAskTimes.push(performance.now())
        return endpoint === null ? Promise.reject(notReady) : Promise.resolve(endpoint)
      }
      if (command === "load_surface_credential") {
        asked[command]++
        return credential === null
          ? Promise.reject(notProvisioned)
          : Promise.resolve(credential)
      }
      // Where the page asked the window's controls to stand, in order, as
      // `traffic_lights.rs` would place them.
      if (command === "lead_window_controls") {
        window.__fakeHostLeads.push(args?.by)
        return Promise.resolve()
      }
      return new Promise(() => {})
    },
  }
}
