import { forwardWebviewConsole, type WebviewConsoleEntry } from "../host/window"

type ConsoleWriter = Pick<Console, "warn" | "error">
type Forward = (entry: WebviewConsoleEntry) => Promise<void> | void

function describe(value: unknown): string {
  try {
    if (value instanceof Error) return value.stack ?? value.message
    if (typeof value === "string") return value
    const json = JSON.stringify(value)
    if (json !== undefined) return json
    return String(value)
  } catch {
    return "[unprintable value]"
  }
}

/**
 * Keeps the page console intact while mirroring warnings and errors to the
 * terminal that started a development desktop app.
 */
export function installDevConsoleForwarding(
  target: ConsoleWriter = console,
  forward: Forward = forwardWebviewConsole,
): () => void {
  const originals = { warn: target.warn, error: target.error }

  for (const level of ["warn", "error"] as const) {
    target[level] = (...values: unknown[]) => {
      Reflect.apply(originals[level], target, values)
      try {
        const entry = {
          level,
          // Four UTF-8 bytes per JavaScript character is the worst case, so
          // these limits fit the host's byte bounds before the IPC allocates.
          message: values
            .map(describe)
            .join(" ")
            .slice(0, 4 * 1024),
          source: (new Error().stack ?? "source unavailable").slice(0, 2 * 1024),
        }
        void Promise.resolve(forward(entry)).catch(() => undefined)
      } catch {
        // Diagnostics must never turn a page warning into an application fault.
      }
    }
  }

  return () => {
    target.warn = originals.warn
    target.error = originals.error
  }
}
