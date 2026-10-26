import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { test } from "node:test"
import { observeGatewayRestarts } from "./stress-restart.mjs"
import { watchLines } from "./page-lines.mjs"

const endpoint = "ws://127.0.0.1:12345"
const REFUSED = "Could not connect to 127.0.0.1: Connection refused"
const RESET = "Error receiving data: Connection reset by peer"
const LOST = "The network connection was lost."
const CONNECT = "Could not connect to the server."
const request = (ws, id = "read", method = "conversation.subscribeList") =>
  ws.emit("framesent", { payload: JSON.stringify({ type: "req", id, method }) })
const response = (ws, id = "read", ok = true) =>
  ws.emit("framereceived", { payload: JSON.stringify({ type: "res", id, ok }) })

for (const method of ["conversation.subscribeList", "conversation.subscribe"]) {
  test(`browser recovery requires its new ${method} acknowledgement`, async () => {
    const f = fixture()
    await f.observer.during(f.opened, async () => {
      const ws = f.socket()
      const ready = f.observer.readable(10_000)
      request(ws, "read", method)
      response(ws)
      assert.equal(await ready, true)
      return true
    })
    f.observer.dispose()
  })
}

for (const [name, evidence] of [
  [
    "challenge alone",
    (f, ws) =>
      ws.emit("framereceived", {
        payload: JSON.stringify({ type: "event", event: "session.challenge" }),
      }),
  ],
  [
    "authentication alone",
    (f, ws) => {
      request(ws, "read", "session.authenticate")
      response(ws)
    },
  ],
  [
    "failed read",
    (f, ws) => {
      request(ws)
      response(ws, "read", false)
    },
  ],
  ["unsolicited reply", (f, ws) => response(ws)],
  [
    "another socket's reply",
    (f, ws) => {
      request(ws)
      response(f.socket())
    },
  ],
  [
    "closed acknowledged socket",
    (f, ws) => {
      request(ws)
      response(ws)
      ws.emit("close")
    },
  ],
  [
    "failed acknowledged socket",
    (f, ws) => {
      request(ws)
      response(ws)
      f.fail(ws)
    },
  ],
]) {
  test(`${name} cannot establish usable browser recovery`, async (t) => {
    t.mock.timers.enable({ apis: ["setTimeout"] })
    const f = fixture()
    await f.observer.during(f.opened, async () => {
      evidence(f, f.socket())
      const ready = f.observer.readable(10_000)
      t.mock.timers.tick(10_000)
      assert.equal(await ready, false)
      return false
    })
    f.observer.dispose()
  })
}

test("stalled recovery exhausts the existing Send budget rather than sending", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] })
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    const ws = f.socket()
    request(ws)
    let settled = false
    const ready = f.observer.readable(10_000).then((value) => {
      settled = true
      return value
    })
    t.mock.timers.tick(9_999)
    await Promise.resolve()
    assert.equal(settled, false)
    t.mock.timers.tick(1)
    assert.equal(await ready, false)
    return false
  })
  f.observer.dispose()
})

for (const count of [64, 65]) {
  test(`${count} pending workspace reads ${count === 64 ? "allow" : "refuse"} readiness`, async (t) => {
    t.mock.timers.enable({ apis: ["setTimeout"] })
    const f = fixture()
    await f.observer.during(f.opened, async () => {
      const ws = f.socket()
      for (let index = 0; index < count; index++) request(ws, String(index))
      response(ws, "0")
      const ready = f.observer.readable(10_000)
      t.mock.timers.tick(10_000)
      assert.equal(await ready, count === 64)
      return count === 64
    })
    f.observer.dispose()
  })
}

test("the pre-restart socket's read does not prove a replacement recovered", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] })
  const f = fixture()
  const old = f.socket()
  await f.observer.during(f.opened, async () => {
    request(old)
    response(old)
    const ready = f.observer.readable(10_000)
    t.mock.timers.tick(10_000)
    assert.equal(await ready, false)
    return false
  })
  f.observer.dispose()
})

test("a socket closing before a recovered waiter resumes is refused", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    const ws = f.socket()
    const ready = f.observer.readable(10_000)
    request(ws)
    response(ws)
    ws.emit("close")
    assert.equal(await ready, false)
    return false
  })
  f.observer.dispose()
})

for (const older of [true, false]) {
  test(`an error on ${older ? "an older" : "another"} socket preserves the live read`, async () => {
    const f = fixture()
    const old = older ? f.socket() : null
    await f.observer.during(f.opened, async () => {
      const ws = f.socket()
      request(ws)
      response(ws)
      f.fail(old ?? f.socket())
      assert.equal(await f.observer.readable(10_000), true)
      return false
    })
    // Readiness is not permission to exempt the refused connection's console.
    assert.equal(f.lines.errors.length, 1)
    assert.equal(f.lines.harmless.length, 0)
    f.observer.dispose()
  })
}

test("late console text cannot revoke another socket's acknowledged read", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    const ws = f.socket()
    request(ws)
    response(ws)
    f.consoleError()
    assert.equal(await f.observer.readable(10_000), true)
    return false
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 0)
  f.observer.dispose()
})

test("more frames on the same socket cannot erase a later console failure", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    const ws = f.socket()
    ws.emit("framereceived", {})
    f.consoleError()
    request(ws)
    response(ws)
    assert.equal(await f.observer.readable(10_000), true)
    return true
  })
  assert.equal(f.lines.errors.length, 2)
  assert.equal(f.lines.harmless.length, 0)
  f.observer.dispose()
})

test("disposing or ending the observation releases a pending readiness waiter", async () => {
  for (const dispose of [true, false]) {
    const f = fixture()
    let ready
    await f.observer.during(f.opened, async () => {
      ready = f.observer.readable(10_000)
      if (dispose) f.observer.dispose()
      return false
    })
    assert.equal(await ready, false)
    f.observer.dispose()
  }
})

function fixture(defaultCause = REFUSED) {
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
  const consoleError = (cause = defaultCause, url = endpoint + "/session") => {
    const text = `WebSocket connection to '${url}' failed: ${cause}`
    page.emit("console", {
      type: () => "error",
      text: () => text,
      location: () => ({}),
    })
  }
  const fail = (ws, cause = defaultCause) => {
    consoleError(cause, ws.url())
    ws.emit("socketerror", cause)
  }
  const recover = () => socket().emit("framereceived", {})
  return { page, lines, opened, observer, socket, consoleError, fail, recover }
}

for (const cause of [
  REFUSED,
  RESET,
  LOST,
  CONNECT,
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

for (const cause of [REFUSED, LOST, CONNECT]) {
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
    test(`${name} (${cause}) leaves every console error failed`, async () => {
      const f = fixture(cause)
      await f.observer.during(f.opened, async () => body(f))
      assert.equal(f.lines.harmless.length, 0)
      assert.ok(f.lines.errors.length > 0)
      f.observer.dispose()
    })
  }
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
    f.fail(f.socket())
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

for (const [name, body] of [
  [
    "receiving socket resets and closes",
    (f) => {
      const socket = f.socket()
      socket.emit("framereceived", {})
      f.fail(socket, RESET)
      socket.emit("close")
    },
  ],
  [
    "first recovery fails again",
    (f) => {
      f.fail(f.socket())
      const socket = f.socket()
      socket.emit("framereceived", {})
      f.fail(socket, RESET)
      socket.emit("close")
    },
  ],
  [
    "receiving socket closes without another error",
    (f) => {
      f.fail(f.socket())
      const socket = f.socket()
      socket.emit("framereceived", {})
      socket.emit("close")
    },
  ],
  [
    "failed socket emits a buffered receiving frame",
    (f) => {
      const socket = f.socket()
      f.fail(socket)
      socket.emit("framereceived", {})
    },
  ],
  [
    "console failure arrives after receiving proof",
    (f) => {
      f.socket().emit("socketerror", REFUSED)
      f.recover()
      f.consoleError()
    },
  ],
  [
    "later unknown socket error invalidates earlier recovery",
    (f) => {
      f.fail(f.socket())
      const socket = f.socket()
      socket.emit("framereceived", {})
      socket.emit("socketerror", "TLS certificate expired")
    },
  ],
]) {
  test(name + " needs a later live receiving connection", async () => {
    const f = fixture()
    await f.observer.during(f.opened, async () => {
      body(f)
      return true
    })
    assert.ok(f.lines.errors.length > 0)
    assert.equal(f.lines.harmless.length, 0)
    f.observer.dispose()
  })
}

test("a later distinct live receiving socket recovers both outages", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    const first = f.socket()
    first.emit("framereceived", {})
    f.fail(first, RESET)
    first.emit("close")
    f.recover()
    return true
  })
  assert.equal(f.lines.errors.length, 0)
  assert.equal(f.lines.harmless.length, 2)
  f.observer.dispose()
})

test("closing an older failed socket preserves the distinct live recovery", async () => {
  const f = fixture()
  await f.observer.during(f.opened, async () => {
    const old = f.socket()
    f.fail(old)
    f.recover()
    old.emit("close")
    return true
  })
  assert.equal(f.lines.errors.length, 0)
  assert.equal(f.lines.harmless.length, 1)
  f.observer.dispose()
})

test("retired observation cannot exempt a retained socket after listener cleanup fails", async () => {
  const f = fixture()
  let recovered
  let originalOff
  await f.observer.during(f.opened, async () => {
    f.fail(f.socket())
    recovered = f.socket()
    recovered.emit("framereceived", {})
    originalOff = recovered.off
    recovered.off = () => {
      throw new Error("listener cleanup failed")
    }
    assert.throws(() => f.observer.dispose(), /listener cleanup failed/)
    return true
  })
  assert.equal(f.lines.errors.length, 1)
  assert.equal(f.lines.harmless.length, 0)
  recovered.off = originalOff
  f.observer.dispose()
  assert.equal(recovered.listenerCount("socketerror"), 0)
})
