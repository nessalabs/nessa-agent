import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { test } from "node:test"
import { observeGatewayRestarts } from "./stress-restart.mjs"
import { watchLines } from "./page-lines.mjs"

const endpoint = "ws://127.0.0.1:12345"
const REFUSED = "Could not connect to 127.0.0.1: Connection refused"
const RESET = "Error receiving data: Connection reset by peer"
function fixture() {
  const page = new EventEmitter()
  const lines = watchLines()
  page.on("console", (message) => {
    if (message.type() === "error") lines.keep(`console.error: ${message.text()}`, false)
  })
  const opened = {
    page,
    reclassifyHeld: (...args) => lines.reclassifyHeld(...args),
  }
  const observer = observeGatewayRestarts(page, endpoint)
  const socket = (url = endpoint + "/session") => {
    const ws = new EventEmitter()
    ws.url = () => url
    page.emit("websocket", ws)
    return ws
  }
  const consoleError = (cause = REFUSED, url = endpoint + "/session") => {
    const text = `WebSocket connection to '${url}' failed: ${cause}`
    page.emit("console", {
      type: () => "error",
      text: () => text,
      location: () => ({}),
    })
  }
  const fail = (ws, cause = REFUSED) => {
    consoleError(cause, ws.url())
    ws.emit("socketerror", cause)
  }
  const recover = () => socket().emit("framereceived", {})
  return { page, lines, opened, observer, socket, consoleError, fail, recover }
}

for (const cause of [
  REFUSED,
  RESET,
  "Error in connection establishment: net::ERR_CONNECTION_REFUSED",
]) {
  test(`exact owned ${cause} is retained as harmless only after recovery`, async () => {
    const f = fixture()
    const original = f.socket()
    await f.observer.during(f.opened, async () => {
      f.fail(original, cause)
      f.recover()
      return true
    })
    assert.deepEqual(f.lines.errors, [])
    assert.equal(f.lines.harmless.length, 1)
    assert.match(f.lines.harmless[0], /owned gateway restart; session reconnected/)
    f.observer.dispose()
    assert.equal(original.listenerCount("socketerror"), 0)
    assert.equal(f.page.listenerCount("websocket"), 0)
  })
}

test("older identical text keeps every occurrence failed", async () => {
  const f = fixture()
  f.consoleError()
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    f.recover()
    return true
  })
  f.consoleError()
  assert.equal(f.lines.errors.length, 3)
  assert.equal(f.lines.harmless.length, 0)
  f.observer.dispose()
})

test("an identical error after verified recovery is still a failure", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    f.recover()
    return true
  })
  f.consoleError()
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 1)
  f.observer.dispose()
})

for (const [name, body] of [
  [
    "console without socket evidence",
    (f) => {
      f.consoleError()
      f.recover()
      return true
    },
  ],
  [
    "socket without a new receiving connection",
    (f) => {
      f.fail(f.socket())
      return true
    },
  ],
  [
    "failed recovery assertion",
    (f) => {
      f.fail(f.socket())
      f.recover()
      return false
    },
  ],
  [
    "unknown socket cause",
    (f) => {
      f.fail(f.socket(), "TLS certificate expired")
      f.recover()
      return true
    },
  ],
  [
    "another gateway",
    (f) => {
      f.fail(f.socket("ws://127.0.0.1:54321/session"))
      f.recover()
      return true
    },
  ],
  [
    "another path",
    (f) => {
      f.fail(f.socket(endpoint + "/other"))
      f.recover()
      return true
    },
  ],
  [
    "query suffix",
    (f) => {
      f.fail(f.socket(endpoint + "/session?other"))
      f.recover()
      return true
    },
  ],
  [
    "evidence overflow",
    (f) => {
      const ws = f.socket()
      for (let i = 0; i < 65; i++) f.fail(ws)
      f.recover()
      return true
    },
  ],
]) {
  test(`${name} leaves every console error failed`, async () => {
    const f = fixture()
    await f.observer.during(f.opened, async () => body(f))
    assert.equal(f.lines.harmless.length, 0)
    assert.ok(f.lines.errors.length > 0)
    f.observer.dispose()
  })
}

test("throw retains error evidence and retires the observation before another action", async () => {
  const f = fixture()
  await assert.rejects(
    f.observer.during(f.opened, async () => {
      f.fail(f.socket())
      f.recover()
      throw new Error("recovery broke")
    }),
    /recovery broke/,
  )
  await f.observer.during(f.opened, async () => {
    f.recover()
    return true
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 0)
  assert.equal(f.page.listenerCount("console"), 1)
  f.observer.dispose()
})

test("socket evidence is consumed once and cannot pay a second identical console error", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    f.consoleError()
    f.recover()
    return true
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 1)
  f.observer.dispose()
})

test("an earlier connection receiving another frame is not a new connection", async () => {
  const f = fixture()
  const ws = f.socket()
  await f.observer.during(f.opened, async () => {
    f.fail(ws)
    ws.emit("framereceived", {})
    return true
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 0)
  f.observer.dispose()
})

test("another page, nested action and disposed owner reject before running effects", async () => {
  const f = fixture()
  let effects = 0
  await assert.rejects(
    f.observer.during({ ...f.opened, page: new EventEmitter() }, async () => {
      effects++
    }),
    /not available/,
  )
  await f.observer.during(f.opened, async () => {
    await assert.rejects(
      f.observer.during(f.opened, async () => {
        effects++
      }),
      /not available/,
    )
    return false
  })
  f.observer.dispose()
  await assert.rejects(
    f.observer.during(f.opened, async () => {
      effects++
    }),
    /not available/,
  )
  assert.equal(effects, 0)
})
for (const [name, consoleUrl, socketUrl, consoleCause, socketCause] of [
  [
    "owned console with foreign socket",
    endpoint + "/session",
    "ws://127.0.0.1:54321/session",
    REFUSED,
    REFUSED,
  ],
  [
    "foreign console with owned socket",
    "ws://127.0.0.1:54321/session",
    endpoint + "/session",
    REFUSED,
    REFUSED,
  ],
  [
    "owned URL with contradictory causes",
    endpoint + "/session",
    endpoint + "/session",
    RESET,
    REFUSED,
  ],
]) {
  test(name + " cannot create matching restart evidence", async () => {
    const f = fixture()
    await f.observer.during(f.opened, async () => {
      f.consoleError(consoleCause, consoleUrl)
      f.socket(socketUrl).emit("socketerror", socketCause)
      f.recover()
      return true
    })
    assert.equal(f.lines.harmless.length, 0)
    assert.equal(f.lines.errors.length, 1)
    f.observer.dispose()
  })
}

test("overflow of active owned sockets stays failed after they close", async () => {
  const f = fixture()
  const sockets = Array.from({ length: 65 }, () => f.socket())
  await f.observer.during(f.opened, async () => {
    f.fail(sockets[0])
    for (const socket of sockets.slice(1)) socket.emit("close")
    f.recover()
    return true
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 0)
  f.observer.dispose()
  assert.equal(sockets[0].listenerCount("socketerror"), 0)
})

test("disposal during recovery cannot publish an exemption", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    f.recover()
    f.observer.dispose()
    return true
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 0)
})

test("more than64 earlier distinct lines conservatively forbid reclassification", async () => {
  const f = fixture()
  for (let i = 0; i < 65; i++) f.lines.keep("earlier unrelated failure " + i, false)
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    f.recover()
    return true
  })
  assert.equal(f.lines.errors.length, 66)
  assert.equal(f.lines.harmless.length, 0)
  f.observer.dispose()
})
