/**
 * Asks the local network, over multicast DNS, who offers a DNS-SD service,
 * the way a phone browses for a gateway taking pairings: one PTR question to
 * 224.0.0.251:5353, then every answer's SRV and TXT records read back.
 * Resolves the instances heard within `ms`, each with its port and TXT
 * values. macOS uses its system Bonjour browser; other hosts use `dgram`.
 */
import dgram from "node:dgram"
import { execFile } from "node:child_process"

/**
 * macOS drops raw multicast from unapproved background binaries. Like the
 * gateway's SystemAnnouncer, browse through mDNSResponder instead. `-Z`
 * resolves the service to SRV/TXT zone records; the escaped instance is an
 * opaque key, so names with spaces stay correlated with their own gateway.
 */
function systemBrowse(service, ms) {
  return new Promise((resolve, reject) => {
    execFile(
      "/usr/bin/dns-sd",
      ["-Z", service.replace(/\.local\.?$/, ""), "local."],
      {
        timeout: ms,
        maxBuffer: 1024 * 1024,
      },
      (error, stdout) => {
        // dns-sd browses until stopped. Only our bounded stop is expected;
        // launch failures and premature exits must not look like an empty LAN.
        if (!error?.killed || error.signal !== "SIGTERM") {
          reject(error ?? new Error("dns-sd stopped before the browse deadline"))
          return
        }
        const found = new Map()
        for (const line of stdout.split("\n")) {
          const srv = line.match(/^(\S+)\s+SRV\s+\d+\s+\d+\s+(\d+)\s/)
          if (srv) {
            const [, instance, port] = srv
            const entry = found.get(instance) ?? { instance, txt: {} }
            entry.port = Number(port)
            found.set(instance, entry)
          }
          const txt = line.match(/^(\S+)\s+TXT\s+"gw=([^"\\]+)"\s*$/)
          if (txt) {
            const [, instance, gateway] = txt
            const entry = found.get(instance) ?? { instance, txt: {} }
            entry.txt.gw = gateway
            found.set(instance, entry)
          }
        }
        resolve([...found.values()].filter((each) => each.port !== undefined))
      },
    )
  })
}

/** `name` as DNS labels. */
function encodeName(name) {
  const parts = name.replace(/\.$/, "").split(".")
  const bytes = []
  for (const part of parts) {
    const label = Buffer.from(part, "utf8")
    bytes.push(label.length, ...label)
  }
  bytes.push(0)
  return Buffer.from(bytes)
}

/** The name at `offset`, following compression pointers; and where it ends. */
function readName(packet, offset) {
  const labels = []
  let end = -1
  for (let guard = 0; guard < 64; guard++) {
    const length = packet[offset]
    if (length === undefined) throw new Error("truncated name")
    if (length === 0) {
      if (end < 0) end = offset + 1
      return { name: labels.join("."), end }
    }
    if ((length & 0xc0) === 0xc0) {
      if (end < 0) end = offset + 2
      offset = ((length & 0x3f) << 8) | packet[offset + 1]
      continue
    }
    labels.push(packet.toString("utf8", offset + 1, offset + 1 + length))
    offset += 1 + length
  }
  throw new Error("name loops")
}

/** Every resource record in a response, with its type and raw data offset. */
function records(packet) {
  const questions = packet.readUInt16BE(4)
  const count = packet.readUInt16BE(6) + packet.readUInt16BE(8) + packet.readUInt16BE(10)
  let offset = 12
  for (let i = 0; i < questions; i++) offset = readName(packet, offset).end + 4
  const out = []
  for (let i = 0; i < count; i++) {
    const { name, end } = readName(packet, offset)
    const type = packet.readUInt16BE(end)
    const length = packet.readUInt16BE(end + 8)
    out.push({ name, type, data: end + 10, length })
    offset = end + 10 + length
  }
  return out
}

const PTR = 12
const TXT = 16
const SRV = 33

/**
 * The instances of `service` (such as `_nessa-link._tcp.local`) heard on the
 * network within `ms`: `{ instance, port, txt }` each.
 */
export function browse(service, ms = 3_000) {
  if (process.platform === "darwin") return systemBrowse(service, ms)
  return new Promise((resolve) => {
    const found = new Map()
    const socket = dgram.createSocket({ type: "udp4", reuseAddr: true })
    let done = false
    let timer
    // Once only: an error after bind and the timer would both end it.
    const finish = () => {
      if (done) return
      done = true
      clearInterval(asking)
      clearTimeout(timer)
      try {
        socket.close()
      } catch {
        // Already closed by the error that ended it.
      }
      resolve([...found.values()].filter((each) => each.port !== undefined))
    }
    socket.on("error", finish)
    socket.on("message", (packet) => {
      try {
        for (const record of records(packet)) {
          if (record.type === PTR && record.name === service.replace(/\.$/, "")) {
            const instance = readName(packet, record.data).name
            if (!found.has(instance)) found.set(instance, { instance, txt: {} })
          }
        }
        for (const record of records(packet)) {
          const entry = found.get(record.name)
          if (!entry) continue
          if (record.type === SRV) entry.port = packet.readUInt16BE(record.data + 4)
          if (record.type === TXT) {
            let at = record.data
            while (at < record.data + record.length) {
              const length = packet[at]
              const pair = packet.toString("utf8", at + 1, at + 1 + length)
              const split = pair.indexOf("=")
              if (split > 0) entry.txt[pair.slice(0, split)] = pair.slice(split + 1)
              at += 1 + length
            }
          }
        }
      } catch {
        // Another host's packet this reader does not follow.
      }
    })
    const question = Buffer.concat([
      // id 0, standard query, one question
      Buffer.from([0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]),
      encodeName(service),
      Buffer.from([0, PTR, 0, 1]),
    ])
    const ask = () => socket.send(question, 5353, "224.0.0.251", () => {})
    let asking
    socket.bind(5353, () => {
      try {
        socket.addMembership("224.0.0.251")
        socket.setMulticastLoopback(true)
      } catch {
        // No multicast here: nothing will be heard.
      }
      ask()
      // Asked again: a responder that started a moment ago may miss the first.
      asking = setInterval(ask, 1_000)
      timer = setTimeout(finish, ms)
    })
  })
}
