/** Node-side, bounded consumed-byte evidence. Credentials stay in these closures. */
import { createHash } from "node:crypto"

const collectors = new WeakMap()

/** Classification consumes a branded collector, never caller-supplied claims. */
export function resourceReadState(evidence, request) {
  return collectors.get(evidence)?.(request) ?? "invalid"
}

export function resourceEndpoint(request) {
  try {
    const url = new URL(request.url())
    return (
      request.method() === "GET" &&
      ["http:", "https:"].includes(url.protocol) &&
      url.pathname === "/mcp-resources" &&
      !url.search &&
      !url.hash &&
      !url.username &&
      !url.password
    )
  } catch {
    return false
  }
}

/** Hold possible resource failures before exact endpoint eligibility is known. */
export function resourceCandidate(request) {
  try {
    return new URL(request.url()).pathname === "/mcp-resources"
  } catch {
    return false
  }
}

/** A page generation owns this collector; no URL/time/string joins are allowed. */
export function resourceReadEvidence(generation) {
  const tickets = new Map()
  const reads = new Map()
  const requests = new Map()
  let sequence = 0
  let failed = false
  let cutoff = false
  let total = 0
  let activation = null
  let disposed = false
  const ticketState = (ticket) => {
    if (!tickets.has(ticket)) {
      if (tickets.size >= 768) {
        failed = true
        return null
      }
      tickets.set(ticket, { read: null, requests: [], ambiguous: false })
    }
    return tickets.get(ticket)
  }
  const invalidate = () => {
    failed = true
  }
  const collector = {
    invalidate,
    observeRequest(request) {
      if (disposed) return false
      if (!resourceEndpoint(request)) return false
      if (requests.has(request)) return true
      if (cutoff || requests.size >= 512) {
        failed = true
        return true
      }
      let ticket
      try {
        ticket = request.headers()["x-nessa-resource-ticket"]
      } catch {
        /* Missing identity fails, never inferred from URL. */
      }
      const state =
        typeof ticket === "string" && /^[A-Za-z0-9_-]{43}$/.test(ticket)
          ? ticketState(ticket)
          : null
      requests.set(request, state)
      if (state) {
        state.requests.push(request)
        if (state.requests.length !== 1) state.ambiguous = true
      }
      return true
    },
    receive(event) {
      if (disposed) return
      if (
        !event ||
        typeof event !== "object" ||
        event.generation !== generation ||
        !Number.isSafeInteger(event.sequence) ||
        event.sequence !== sequence + 1 ||
        cutoff ||
        event.sequence > 4097
      ) {
        failed = true
        return
      }
      sequence = event.sequence
      const shapes = {
        begin: ["read", "owner", "ticket", "size", "digest"],
        chunk: ["read", "bytes"],
        eof: ["read"],
        transport: ["read"],
        accepted: ["read"],
        live: ["read"],
        failed: ["read"],
        cutoff: ["failed", "activation"],
      }
      if (!Object.hasOwn(shapes, event.type)) {
        failed = true
        return
      }
      const keys = ["generation", "sequence", "type", ...shapes[event.type]]
      if (
        Object.keys(event).length !== keys.length ||
        !keys.every((key) => Object.hasOwn(event, key))
      ) {
        failed = true
        return
      }
      if (event.type === "cutoff") {
        cutoff = true
        activation = event.activation
        if (
          event.failed !== false ||
          !activation ||
          Object.keys(activation).length !== 3 ||
          !["bridge", "api", "transport"].every(
            (key) =>
              Object.hasOwn(activation, key) &&
              Number.isSafeInteger(activation[key]) &&
              activation[key] > 0 &&
              activation[key] <= 256,
          )
        )
          failed = true
        return
      }
      if (!Number.isSafeInteger(event.read) || event.read < 1 || event.read > 256) {
        failed = true
        return
      }
      if (event.type === "begin") {
        if (
          reads.has(event.read) ||
          reads.size >= 256 ||
          !Number.isSafeInteger(event.owner) ||
          event.owner < 1 ||
          event.owner > 128 ||
          typeof event.ticket !== "string" ||
          !/^[A-Za-z0-9_-]{43}$/.test(event.ticket) ||
          !Number.isSafeInteger(event.size) ||
          event.size < 0 ||
          event.size > 4194304 ||
          typeof event.digest !== "string" ||
          !/^[a-f0-9]{64}$/.test(event.digest)
        ) {
          failed = true
          return
        }
        const state = ticketState(event.ticket)
        if (!state || state.read) {
          failed = true
          return
        }
        const read = {
          size: event.size,
          digest: event.digest,
          hash: createHash("sha256"),
          length: 0,
          phase: "reading",
          failed: false,
        }
        reads.set(event.read, read)
        state.read = read
        return
      }
      const read = reads.get(event.read)
      if (!read) {
        failed = true
        return
      }
      if (event.type === "failed") {
        read.failed = true
        return
      }
      if (read.failed) return
      switch (event.type) {
        case "chunk": {
          const bytes = event.bytes
          if (
            read.phase !== "reading" ||
            !Array.isArray(bytes) ||
            bytes.length > 4194304 ||
            read.length + bytes.length > read.size ||
            total + bytes.length > 67108864 ||
            !bytes.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255)
          ) {
            read.failed = true
            return
          }
          read.length += bytes.length
          total += bytes.length
          read.hash.update(Buffer.from(bytes))
          break
        }
        case "eof":
          if (
            read.phase !== "reading" ||
            read.length !== read.size ||
            read.hash.digest("hex") !== read.digest
          )
            read.failed = true
          else read.phase = "eof"
          read.hash = null
          break
        case "transport":
          if (read.phase !== "eof") read.failed = true
          else read.phase = "transport"
          break
        case "accepted":
          if (read.phase !== "transport") read.failed = true
          else read.phase = "accepted"
          break
        case "live":
          if (read.phase !== "accepted" && read.phase !== "live") read.failed = true
          else read.phase = "live"
          break
      }
    },
    readState(request) {
      if (disposed || failed || !cutoff || !requests.has(request)) return "invalid"
      const state = requests.get(request)
      if (!state || state.ambiguous) return "invalid"
      if (!state.read) return "unobserved"
      const read = state.read
      return !read.failed && read.phase === "live" && state.requests[0] === request
        ? "verified"
        : "invalid"
    },
    summary() {
      let verified = 0
      for (const request of requests.keys())
        if (this.readState(request) === "verified") verified++
      return {
        cutoff,
        failed,
        activation,
        reads: reads.size,
        requests: requests.size,
        verified,
      }
    },
    dispose() {
      disposed = true
      tickets.clear()
      reads.clear()
      requests.clear()
    },
  }
  collectors.set(collector, collector.readState)
  return Object.freeze(collector)
}
