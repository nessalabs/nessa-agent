/**
 * The desktop app's host, faked for a page: what `gateway-states.mjs` and
 * `gateway-window.mjs` run the real frontend against, so `host.kind` is
 * native and `main.tsx` composes `hostGateway` over IPC.
 */

/**
 * Installed before the page's own scripts, as Tauri installs its IPC. The
 * endpoint and credential commands answer as the scenario says, and are
 * counted (the endpoint's asks timed too); every other command waits
 * forever, as an unanswered host does.
 */
export function gatewayHost({ endpoint, credential }) {
  // The host's own sentences (`GatewayReader::ready`, `CredentialRefusal::NotProvisioned`).
  const notReady = "The local server isn't ready yet"
  const notProvisioned =
    "No chat credential has been provisioned yet. Start the local server " +
    "(`just start`, or `just server`), which creates one on first run."
  let callbacks = 0
  const asked = { load_gateway_endpoint: 0, load_surface_credential: 0 }
  window.__fakeHostAsked = asked
  // When each endpoint ask came, in the page's `performance.now()`.
  window.__fakeHostAskTimes = []
  window.__TAURI_INTERNALS__ = {
    transformCallback: () => ++callbacks,
    invoke(command) {
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
      return new Promise(() => {})
    },
  }
}
