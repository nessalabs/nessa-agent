/** Browser-only observation of original resource reads; never drives the app. */
export function createResourceObserver(deliver, generation) {
  const limits = {
    mounts: 128,
    reads: 256,
    chunk: 4194304,
    bytes: 67108864,
    queued: 128,
    queuedBytes: 8388608,
    messages: 4096,
  }
  const owners = new WeakMap()
  const reads = new Map()
  const mounts = []
  const removers = []
  const activation = { bridge: 0, api: 0, transport: 0 }
  let sequence = 0
  let queued = 0
  let queuedBytes = 0
  let totalBytes = 0
  let failed = false
  let cutoff = false
  let chain = Promise.resolve()
  let finalized
  const observe = (promise, good, bad) => {
    // Always return the application's exact Promise. Observer reactions cannot
    // throw into a derived, unhandled rejection or reject the original read.
    promise
      .then(
        (value) => {
          try {
            good(value)
          } catch {
            failed = true
          }
        },
        () => {
          try {
            bad()
          } catch {
            failed = true
          }
        },
      )
      .catch(() => {
        failed = true
      })
    return promise
  }
  const emit = (event, byteLength = 0) => {
    if (cutoff || failed) return
    if (
      queued >= limits.queued ||
      queuedBytes + byteLength > limits.queuedBytes ||
      sequence >= limits.messages
    ) {
      failed = true
      return
    }
    queued++
    queuedBytes += byteLength
    const message = { generation, sequence: ++sequence, ...event }
    chain = chain
      .then(() => deliver(message))
      .catch(() => {
        failed = true
      })
      .finally(() => {
        queued--
        queuedBytes -= byteLength
      })
  }
  const fail = (read) => {
    if (read && !read.failed) {
      read.failed = true
      read.bytes = null
      emit({ type: "failed", read: read.id })
    }
  }
  const publishLive = (read) => {
    // Acceptance and original-owner liveness can arrive in either order.
    // resource-read-liveness.test.mjs covers both orders and irreversible stop.
    if (!read.failed && read.accepted && read.mount.live && !read.mount.signal?.aborted)
      emit({ type: "live", read: read.id })
  }
  const signal = (value, cancel) => {
    if (!value || typeof value.addEventListener !== "function") {
      cancel()
      return
    }
    if (value.aborted) cancel()
    value.addEventListener("abort", cancel, { once: true })
    removers.push(() => value.removeEventListener("abort", cancel))
  }
  const wrapTransport = (original) =>
    function (...args) {
      activation.transport++
      const fetch = args[1]
      args[1] = function (...fetchArgs) {
        const init = fetchArgs[1]
        const ticket = init?.headers?.["x-nessa-resource-ticket"]
        const read = !cutoff && !failed ? reads.get(ticket) : null
        if (!read || read.failed) {
          if (!cutoff && !failed && !read) failed = true
          return Reflect.apply(fetch, this, fetchArgs)
        }
        if (read.fetch || init.method !== "GET" || init.signal !== read.innerSignal) {
          fail(read)
          return Reflect.apply(fetch, this, fetchArgs)
        }
        read.fetch = true
        signal(init.signal, () => fail(read))
        const promise = Reflect.apply(fetch, this, fetchArgs)
        return observe(
          promise,
          (response) => {
            if (!read || cutoff) return
            if (response.status !== 200 || !response.body) {
              fail(read)
              return
            }
            const body = response.body
            const getReader = body.getReader
            body.getReader = function (...readerArgs) {
              const reader = Reflect.apply(getReader, this, readerArgs)
              if (read.reader) fail(read)
              read.reader = true
              const next = reader.read
              const cancel = reader.cancel
              reader.cancel = function (...cancelArgs) {
                fail(read)
                return Reflect.apply(cancel, this, cancelArgs)
              }
              reader.read = function (...nextArgs) {
                return observe(
                  Reflect.apply(next, this, nextArgs),
                  ({ done, value }) => {
                    if (cutoff || read.failed) return
                    if (done) {
                      if (read.eof) {
                        fail(read)
                        return
                      }
                      read.eof = true
                      emit({ type: "eof", read: read.id })
                      return
                    }
                    if (
                      read.eof ||
                      !(value instanceof Uint8Array) ||
                      value.byteLength > limits.chunk ||
                      read.length + value.byteLength > read.size ||
                      totalBytes + value.byteLength > limits.bytes ||
                      queued >= limits.queued ||
                      sequence >= limits.messages ||
                      queuedBytes + value.byteLength > limits.queuedBytes
                    ) {
                      fail(read)
                      failed = true
                      return
                    }
                    read.length += value.byteLength
                    totalBytes += value.byteLength
                    // Bounds checked before the sole observer copy/marshalling.
                    emit(
                      { type: "chunk", read: read.id, bytes: Array.from(value) },
                      value.byteLength,
                    )
                  },
                  () => fail(read),
                )
              }
              return reader
            }
          },
          () => fail(read),
        )
      }
      const transport = Reflect.apply(original, this, args)
      const get = transport.get
      transport.get = function (...getArgs) {
        const read = !cutoff && !failed ? reads.get(getArgs[0]?.ticket) : null
        if (!read || read.failed) {
          if (!cutoff && !failed && !read) failed = true
          return Reflect.apply(get, this, getArgs)
        }
        if (getArgs[0].maxBytes !== read.size || read.innerSignal) fail(read)
        read.innerSignal = getArgs[0].signal
        return observe(
          Reflect.apply(get, this, getArgs),
          (reply) => {
            if (!read || cutoff) return
            if (
              read.failed ||
              !read.eof ||
              reply.status !== 200 ||
              read.length !== read.size ||
              !(reply.bytes instanceof Uint8Array) ||
              reply.bytes.byteLength !== read.length
            ) {
              fail(read)
              return
            }
            read.bytes = reply.bytes
            emit({ type: "transport", read: read.id })
          },
          () => fail(read),
        )
      }
      return transport
    }
  const wrapApi = (original) =>
    function (...args) {
      activation.api++
      const api = Reflect.apply(original, this, args)
      const fetch = api.fetchResource
      api.fetchResource = function (ticket, expected, options = {}) {
        let read
        if (!cutoff && !failed) {
          const mount = owners.get(options.signal)
          if (reads.size >= limits.reads) failed = true
          else if (reads.has(ticket)) {
            fail(reads.get(ticket))
            failed = true
          } else if (
            mount &&
            typeof ticket === "string" &&
            /^[A-Za-z0-9_-]{43}$/.test(ticket) &&
            Number.isSafeInteger(expected?.size) &&
            expected.size >= 0 &&
            expected.size <= limits.chunk &&
            /^[a-f0-9]{64}$/.test(expected.sha256)
          ) {
            read = {
              id: reads.size + 1,
              mount,
              size: expected.size,
              length: 0,
              failed: false,
              eof: false,
              fetch: false,
              accepted: false,
            }
            reads.set(ticket, read)
            mount.reads.push(read)
            emit({
              type: "begin",
              read: read.id,
              owner: mount.id,
              ticket,
              size: expected.size,
              digest: expected.sha256,
            })
            if (options.signal.aborted) fail(read)
          } else failed = true
        }
        const promise = Reflect.apply(fetch, this, [ticket, expected, options])
        if (!read) return promise
        return observe(
          promise,
          (bytes) => {
            if (!read || cutoff) return
            if (
              read.failed ||
              !read.eof ||
              read.bytes !== bytes ||
              read.mount.signal.aborted
            ) {
              fail(read)
              return
            }
            // Original API fulfillment is authoritative for wrapper/length/digest.
            read.accepted = true
            read.bytes = null
            emit({ type: "accepted", read: read.id })
            publishLive(read)
          },
          () => fail(read),
        )
      }
      return api
    }
  const wrapBridge = (original) =>
    function (options) {
      activation.bridge++
      if (cutoff || failed || mounts.length >= limits.mounts) {
        failed = true
        return Reflect.apply(original, this, [options])
      }
      const mount = {
        id: mounts.length + 1,
        reads: [],
        signal: null,
        address: null,
        live: false,
      }
      mounts.push(mount)
      const server = options.ports.server
      const wrapped = {
        ...options,
        ports: {
          ...options.ports,
          server: {
            ...server,
            readResource(...args) {
              const [address, , actualSignal] = args
              if (!mount.signal) {
                mount.signal = actualSignal
                mount.address = address
                if (owners.has(actualSignal)) failed = true
                else owners.set(actualSignal, mount)
                signal(actualSignal, () => {
                  mount.live = false
                  for (const read of mount.reads) fail(read)
                })
              } else if (mount.signal !== actualSignal || mount.address !== address) {
                failed = true
                for (const read of mount.reads) fail(read)
              }
              return Reflect.apply(server.readResource, server, args)
            },
          },
        },
        onView(view) {
          try {
            options.onView(view)
          } catch (error) {
            for (const read of mount.reads) fail(read)
            throw error
          }
          if (cutoff) return
          mount.live = view?.lifecycle?.kind === "live"
          if (mount.live) for (const read of mount.reads) publishLive(read)
          else if (["gone", "failed", "ending"].includes(view?.lifecycle?.kind))
            for (const read of mount.reads) fail(read)
        },
      }
      return Reflect.apply(original, this, [wrapped])
    }
  return {
    bridge: wrapBridge,
    api: wrapApi,
    transport: wrapTransport,
    finalize() {
      if (finalized) return finalized
      // Synchronous measurement cutoff: original signals and ownership, before
      // any await or context disposal. Subsequent app activity cannot add proof.
      for (const mount of mounts)
        for (const read of mount.reads)
          if (!mount.live || mount.signal?.aborted) fail(read)
      cutoff = true
      for (const remove of removers) remove()
      const finalSequence = ++sequence
      finalized = chain
        .then(() =>
          deliver({
            generation,
            sequence: finalSequence,
            type: "cutoff",
            failed,
            activation: { ...activation },
          }),
        )
        .finally(() => {
          reads.clear()
          for (const mount of mounts) mount.reads.length = 0
          mounts.length = 0
          removers.length = 0
        })
      return finalized
    },
  }
}
