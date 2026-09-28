/**
 * The Agents overview's filter, kept in this webview's storage between
 * launches (`RememberedFilter`): read once when the window is composed, and
 * written whenever the store's filter changes. The store owns the filter;
 * storage never speaks for it while the window is open.
 *
 * Storage can be missing or refuse access (a private window, cleared site
 * data): reading then gives the default, and a choice still applies to this
 * window, simply not remembered.
 */
import type { RememberedFilter } from "../../application/ports"
import { parseFilter, serializeFilter } from "../../model/overview/filter"

const key = "nessa.desktop.experiments.agents-overview.filter"

export function rememberedFilter(
  storage: () => Pick<Storage, "getItem" | "setItem"> = () => window.localStorage,
): RememberedFilter {
  return {
    read() {
      try {
        return parseFilter(storage().getItem(key))
      } catch {
        return parseFilter(null)
      }
    },
    write(filter) {
      try {
        storage().setItem(key, serializeFilter(filter))
      } catch {
        // Not remembered; the choice still applies to this window.
      }
    },
  }
}
