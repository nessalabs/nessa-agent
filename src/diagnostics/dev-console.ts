import { forwardWebviewConsole, type WebviewConsoleEntry } from "../host/window"

type ConsoleWriter = Pick<Console, "warn" | "error">
type Forward = (entry: WebviewConsoleEntry) => Promise<void> | void
type Source = () => string

/**
 * Repairs malformed UTF-16 and keeps at most `mostScalars` Unicode values.
 *
 * JavaScript permits lone surrogates, while the host's JSON `String` does not.
 * Iterating a string combines a valid surrogate pair into one scalar and leaves
 * a lone surrogate as one code unit, which is replaced before IPC serialization.
 */
function boundedScalarText(value: string, mostScalars: number): string {
  const bounded: string[] = []
  for (const scalar of value) {
    if (bounded.length === mostScalars) break
    const unit = scalar.charCodeAt(0)
    bounded.push(
      scalar.length === 1 && unit >= 0xd800 && unit <= 0xdfff ? "\ufffd" : scalar,
    )
  }
  return bounded.join("")
}

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
  source: Source = () => new Error().stack ?? "source unavailable",
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
          message: boundedScalarText(values.map(describe).join(" "), 4 * 1024),
          source: boundedScalarText(source(), 2 * 1024),
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
