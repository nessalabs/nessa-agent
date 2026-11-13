import { strict as assert } from "node:assert"
import { createHash, randomBytes } from "node:crypto"
import { mkdtempSync, readFileSync, rmSync, truncateSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, dirname, join, resolve } from "node:path"
import { spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"
import { test } from "node:test"
import { budgetMs } from "./perf.mjs"
import {
  desktopReceipt,
  decodeDesktopReceipt,
  incompleteReceipt,
  receiptNotices,
  receiptGroups,
  receiptPartsPerGroup,
  receiptPartSize,
  receiptMaxRawBytes,
  receiptMaxProjectionBytes,
} from "./ci-receipt.mjs"

const source = { commit: "a".repeat(40), run: "12345", attempt: "2" }
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..")
const script = join(root, "verification/desktop/scripts/ci-receipt.mjs")

function example() {
  const run = {
    frames: 45,
    frameClock: { origin: 100, at: 200 },
    maxFrame: 50.1,
    over: 1,
    did: null,
    noLoaf: false,
    before: { pane: "a" },
    after: { pane: "b" },
    slow: [
      {
        at: 20,
        frame: 50.1,
        loaf: { scripts: [{ source: "owner.ts:12", forcedLayout: 9 }] },
      },
    ],
    sample: { gaps: [[20, 50.1]], inputs: [{ key: "x" }] },
  }
  return {
    check: "run-all",
    ok: false,
    summary: [
      { check: "focus", status: "PASS", held: "1/1", seconds: 1 },
      { check: "perf-budget", status: "FAIL", held: "1/2", seconds: 3 },
    ],
    checks: {
      focus: {
        check: "focus",
        ok: true,
        results: [
          {
            name: "move",
            engine: "webkit",
            ok: true,
            failures: [],
            details: { geometry: [1, 2] },
          },
        ],
      },
      "perf-budget": {
        check: "perf-budget",
        ok: false,
        target: { mode: "prod" },
        results: [
          {
            name: "calibration",
            ok: true,
            calibration: { rate: 4, ratio: 3.9 },
            calibrationFrame: { cost: 75, attributed: true },
          },
          {
            name: "sidebar",
            engine: "chromium",
            layout: "columns",
            ok: false,
            max: 50.1,
            median: 49.6,
            over50: 1,
            failures: ["frame over budget"],
            runs: [run],
            baseRuns: [{ ...run, maxFrame: 49.6 }],
          },
        ],
      },
    },
  }
}

function annotations(receipt) {
  return ["manifest", "group1", "group2", "group3"]
    .flatMap((section) => receiptNotices(receipt, section))
    .map((line) => {
      const found = /^::notice title=(.*?)::(.*)$/.exec(line)
      assert.ok(found, line)
      assert.ok(Buffer.byteLength(line, "utf8") < 4096)
      return { title: found[1], message: found[2] }
    })
}

function changeManifest(rows, change) {
  const copy = structuredClone(rows)
  const data = JSON.parse(Buffer.from(copy[0].message, "base64").toString("utf8"))
  change(data)
  copy[0].message = Buffer.from(JSON.stringify(data)).toString("base64")
  return copy
}

test("round trips the producer verdict, exact measurements, calibration and every slow-frame attribution", () => {
  const document = example()
  const raw = JSON.stringify(document, null, 2) + "\n"
  const receipt = desktopReceipt(raw, source)
  assert.equal(receipt.manifest.status, "prepared")
  assert.equal(receipt.manifest.producerOk, false)
  assert.equal(receipt.manifest.rawSha256, createHash("sha256").update(raw).digest("hex"))
  const decoded = decodeDesktopReceipt(annotations(receipt).reverse(), source)
  assert.deepEqual(decoded.projection.summary, document.summary)
  assert.equal(decoded.projection.ok, false)
  assert.equal(decoded.projection.budgetMs, budgetMs)
  const actual = decoded.projection.checks["perf-budget"]
  const expected = structuredClone(document.checks["perf-budget"])
  for (const row of expected.results)
    for (const key of ["runs", "baseRuns"])
      for (const run of row[key] ?? []) delete run.sample
  assert.deepEqual(actual, expected)
  assert.equal(actual.results[1].max, 50.1)
  assert.equal(actual.results[1].runs[0].frames, 45)
  assert.equal(actual.results[1].runs[0].slow[0].loaf.scripts[0].source, "owner.ts:12")
  assert.equal(decoded.projection.checks.focus.results[0].details, undefined)
})

test("retains failed, skipped, cannot-run and contradictory successful records whole", () => {
  const document = example()
  const records = [
    {
      name: "failure without text",
      ok: false,
      failures: [],
      original: { id: "original failure" },
    },
    { name: "failure", ok: false, error: "", failures: [], original: { id: "original" } },
    { name: "skip", ok: true, skipped: "dev only", owner: { stage: "retained" } },
    { name: "cannot", couldNotRun: "missing display", owner: { stage: "opening" } },
    { name: "contradiction", ok: true, failures: ["late"], causal: { id: "a" } },
  ]
  document.checks.focus.results = records
  const decoded = decodeDesktopReceipt(
    annotations(desktopReceipt(JSON.stringify(document), source)),
    source,
  )
  assert.deepEqual(decoded.projection.checks.focus.results, records)
})

test("keeps all five runs and arbitrary published measurement fields without recalculation", () => {
  const document = example()
  const row = document.checks["perf-budget"].results[1]
  row.runs = Array.from({ length: 5 }, (_, i) => ({
    ...row.runs[0],
    maxFrame: 49.6 + i / 10,
    newOwnerField: { id: i },
  }))
  const decoded = decodeDesktopReceipt(
    annotations(desktopReceipt(JSON.stringify(document), source)),
    source,
  )
  const actual = decoded.projection.checks["perf-budget"].results[1]
  assert.equal(actual.runs.length, 5)
  assert.equal(actual.max, row.max)
  assert.deepEqual(
    actual.runs.map((run) => run.maxFrame),
    row.runs.map((run) => run.maxFrame),
  )
  assert.deepEqual(
    actual.runs.map((run) => run.newOwnerField),
    row.runs.map((run) => run.newOwnerField),
  )
})

test("splits incompressible receipts across bounded groups, preserving Unicode and literal commands", () => {
  const document = example()
  document.checks.focus.results = [
    {
      name: "bad",
      ok: false,
      failures: ["雪🙂\n::error::not a command%0A"],
      original: randomBytes(30_000).toString("base64"),
    },
  ]
  const receipt = desktopReceipt(JSON.stringify(document), source)
  assert.equal(receipt.manifest.status, "prepared")
  assert.ok(receipt.parts.length > receiptPartsPerGroup)
  assert.ok(receipt.parts.length <= receiptPartsPerGroup * receiptGroups)
  for (const section of ["group1", "group2", "group3"]) {
    assert.ok(receiptNotices(receipt, section).length <= receiptPartsPerGroup)
    for (const line of receiptNotices(receipt, section)) {
      assert.equal(line.split("\n").length, 1)
      assert.match(line.slice(line.indexOf("::", 2) + 2), /^[A-Za-z0-9+/]*={0,2}$/)
    }
  }
  assert.ok(receipt.parts.every((part) => part.length <= receiptPartSize))
  const decoded = decodeDesktopReceipt(annotations(receipt), source)
  assert.deepEqual(decoded.projection.checks.focus.results, document.checks.focus.results)
})

test("oversize receipts publish an incomplete manifest and zero partial data", () => {
  const document = example()
  document.checks.focus.results = [
    { name: "bad", ok: false, error: randomBytes(100_000).toString("base64") },
  ]
  const receipt = desktopReceipt(JSON.stringify(document), source)
  assert.equal(receipt.manifest.status, "incomplete")
  assert.equal(receipt.manifest.reason, "capacity-exceeded")
  assert.equal(receipt.manifest.producerOk, false)
  assert.ok(receipt.manifest.parts > receiptPartsPerGroup * receiptGroups)
  assert.deepEqual(receipt.parts, [])
  assert.deepEqual(receiptNotices(receipt, "group1"), [])
  assert.throws(() => decodeDesktopReceipt(annotations(receipt), source), /incomplete/)
})

test("missing, duplicate and mixed parts or manifests cannot establish delivery", () => {
  const receipt = desktopReceipt(JSON.stringify(example()), source)
  const rows = annotations(receipt)
  for (const altered of [
    rows.slice(1),
    rows.slice(0, 1),
    [...rows, rows[0]],
    [...rows, rows[1]],
    rows.map((row, i) =>
      i === 1
        ? {
            ...row,
            title: row.title.replace(receipt.manifest.projectionSha256, "b".repeat(64)),
          }
        : row,
    ),
  ])
    assert.throws(() => decodeDesktopReceipt(altered, source))
  for (const key of ["commit", "run", "attempt"])
    assert.throws(
      () =>
        decodeDesktopReceipt(rows, {
          ...source,
          [key]: key === "commit" ? "b".repeat(40) : "99",
        }),
      /source/,
    )
})

test("a partial API page cannot stand in for a complete annotation collection", () => {
  const document = example()
  document.checks.focus.results = [
    { name: "large failure", ok: false, error: randomBytes(30_000).toString("base64") },
  ]
  const receipt = desktopReceipt(JSON.stringify(document), source)
  const rows = [
    ...Array.from({ length: 20 }, () => ({ title: "Native acceptance", message: "ok" })),
    ...annotations(receipt),
  ]
  assert.ok(rows.length > 30)
  assert.throws(() => decodeDesktopReceipt(rows.slice(0, 30), source), /missing/)
  assert.equal(
    decodeDesktopReceipt([...rows.slice(0, 30), ...rows.slice(30)], source).projection.ok,
    false,
  )
})

test("decompression honors the declared byte bound before checking the resulting JSON", () => {
  const rows = annotations(desktopReceipt(JSON.stringify(example()), source))
  assert.throws(
    () =>
      decodeDesktopReceipt(
        changeManifest(rows, (manifest) => {
          manifest.projectionBytes = 1
        }),
        source,
      ),
    (error) => error.code === "ERR_BUFFER_TOO_LARGE",
  )
})

test("digest fields require strings rather than coercible arrays", () => {
  const rows = annotations(desktopReceipt(JSON.stringify(example()), source))
  for (const field of ["rawSha256", "projectionSha256"])
    assert.throws(
      () =>
        decodeDesktopReceipt(
          changeManifest(rows, (manifest) => {
            manifest[field] = [manifest[field]]
          }),
          source,
        ),
      /invalid desktop receipt manifest/,
    )
})

test("the notice selector accepts only the declared groups", () => {
  const receipt = desktopReceipt(JSON.stringify(example()), source)
  for (const group of ["group0", "group4", "group01", "group99999", "other", undefined])
    assert.throws(() => receiptNotices(receipt, group))
})

test("rejects corrupt bytes, false lengths, decompression bounds and mismatched producer facts", () => {
  const rows = annotations(desktopReceipt(JSON.stringify(example()), source))
  const corrupt = structuredClone(rows)
  corrupt[1].message = "!" + corrupt[1].message.slice(1)
  assert.throws(() => decodeDesktopReceipt(corrupt, source), /encoding/)
  const invalidManifest = structuredClone(rows)
  const manifest = JSON.parse(Buffer.from(rows[0].message, "base64").toString("utf8"))
  manifest.diagnostic = "marker"
  const invalidUtf8 = Buffer.from(JSON.stringify(manifest))
  invalidUtf8[invalidUtf8.indexOf("marker")] = 0xff
  invalidManifest[0].message = invalidUtf8.toString("base64")
  assert.throws(() => decodeDesktopReceipt(invalidManifest, source))
  for (const change of [
    (m) => {
      m.projectionSha256 = "c".repeat(64)
    },
    (m) => {
      m.projectionBytes = 1
    },
    (m) => {
      m.projectionBytes = receiptMaxProjectionBytes + 1
    },
    (m) => {
      m.producerOk = !m.producerOk
    },
    (m) => {
      m.omitted = []
    },
    (m) => {
      m.groups += 1
    },
  ])
    assert.throws(() => decodeDesktopReceipt(changeManifest(rows, change), source))
  const wrongDigest = "d".repeat(64)
  const coherentFalseHash = changeManifest(rows, (m) => {
    m.projectionSha256 = wrongDigest
  })
  for (const row of coherentFalseHash.slice(1))
    row.title = row.title.replace(/[a-f0-9]{64}/, wrongDigest)
  assert.throws(
    () => decodeDesktopReceipt(coherentFalseHash, source),
    /content.*manifest/,
  )
  assert.throws(
    () =>
      decodeDesktopReceipt(
        changeManifest(rows, (m) => {
          m.source.attempt = "3"
        }),
        source,
      ),
    /source/,
  )
  const wrongSource = desktopReceipt(JSON.stringify(example()), {
    ...source,
    attempt: "3",
  })
  const falseManifest = changeManifest(annotations(wrongSource), (m) => {
    m.source = source
  })
  assert.throws(() => decodeDesktopReceipt(falseManifest, source), /facts.*manifest/)
})

test("refuses annotation overflow and excess parts before accepting any delivery", () => {
  const receipt = desktopReceipt(JSON.stringify(example()), {
    ...source,
    run: "9".repeat(5000),
  })
  assert.throws(() => receiptNotices(receipt, "manifest"), /capacity/)
  const rows = annotations(desktopReceipt(JSON.stringify(example()), source))
  assert.throws(
    () => decodeDesktopReceipt([...rows, ...Array(25).fill(rows[1])], source),
    /too many/,
  )
})

test("a returned manifest cannot relabel the shared projection contract", () => {
  const raw = JSON.stringify(example())
  const first = desktopReceipt(raw, source)
  const expected = [...first.manifest.omitted]
  first.manifest.omitted.push("changed outside the encoder")
  const next = desktopReceipt(raw, source)
  assert.deepEqual(next.manifest.omitted, expected)
  assert.throws(() => decodeDesktopReceipt(annotations(first), source), /manifest/)
  assert.equal(decodeDesktopReceipt(annotations(next), source).projection.ok, false)
})

test("malformed, absent, oversized and invalid UTF-8 documents remain explicitly incomplete", () => {
  for (const raw of [
    "",
    "{}",
    JSON.stringify({ check: "run-all", ok: true, summary: [] }),
    Buffer.from([0xff]),
  ]) {
    const receipt = desktopReceipt(raw, source)
    assert.equal(receipt.manifest.status, "incomplete")
    assert.deepEqual(receipt.parts, [])
  }
  const oversized = desktopReceipt(Buffer.alloc(receiptMaxRawBytes + 1), source)
  assert.equal(oversized.manifest.reason, "raw-capacity-exceeded")
  assert.deepEqual(oversized.parts, [])
  const protectedSource = incompleteReceipt(source, "missing", {
    source: {},
    status: "prepared",
    kind: "fake",
  })
  assert.deepEqual(protectedSource.manifest.source, source)
  assert.equal(protectedSource.manifest.status, "incomplete")
  assert.throws(() => desktopReceipt("{}", { ...source, run: 123 }), /requires/)
  const invalidUtf8 = Buffer.from(JSON.stringify(example()))
  invalidUtf8[invalidUtf8.indexOf("frame over budget")] = 0xff
  assert.equal(desktopReceipt(invalidUtf8, source).manifest.status, "incomplete")
  const document = example()
  document.checks.focus.results = [
    { name: "large", ok: false, error: "x".repeat(receiptMaxProjectionBytes) },
  ]
  assert.equal(
    desktopReceipt(JSON.stringify(document), source).manifest.reason,
    "projection-capacity-exceeded",
  )
})

test("reports a successful producer without manufacturing a new verdict from copied rows", () => {
  const document = example()
  document.ok = true
  const decoded = decodeDesktopReceipt(
    annotations(desktopReceipt(JSON.stringify(document), source)),
    source,
  )
  assert.equal(decoded.projection.ok, true)
  assert.equal(decoded.manifest.producerOk, true)
  assert.equal(decoded.projection.checks["perf-budget"].ok, false)
  assert.deepEqual(decoded.projection.summary, document.summary)
})

test("does not consume inherited result fields or prototype-like check names", () => {
  const document = example()
  const raw = JSON.stringify(document).replace(
    '"checks":{',
    '"checks":{"__proto__":{"check":"other","results":[]},',
  )
  const decoded = decodeDesktopReceipt(annotations(desktopReceipt(raw, source)), source)
  assert.equal(Object.hasOwn(decoded.projection.checks, "__proto__"), true)
  assert.equal({}.polluted, undefined)
  const inherited = Object.create({
    title: "Desktop verification receipt manifest",
    message: "",
  })
  assert.throws(() => decodeDesktopReceipt([inherited], source), /manifest/)
})

test("real CI entrypoint and workflow groups preserve a failed producer and safely quote paths", () => {
  const dir = mkdtempSync(join(tmpdir(), "desktop receipt "))
  try {
    const file = join(dir, "result file.json")
    writeFileSync(file, JSON.stringify(example()))
    const env = {
      ...process.env,
      GITHUB_SHA: source.commit,
      GITHUB_RUN_ID: source.run,
      GITHUB_RUN_ATTEMPT: source.attempt,
      RUNNER_TEMP: dir,
      PATH: dirname(process.execPath) + delimiter + process.env.PATH,
    }
    const lines = []
    for (const section of ["manifest", "group1", "group2", "group3"]) {
      const run = spawnSync(process.execPath, [script, file, section], {
        env,
        encoding: "utf8",
      })
      assert.equal(run.status, 0, run.stderr)
      lines.push(...run.stdout.trim().split("\n").filter(Boolean))
    }
    const rows = lines.map((line) => {
      const match = /^::notice title=(.*?)::(.*)$/.exec(line)
      return { title: match[1], message: match[2] }
    })
    assert.equal(decodeDesktopReceipt(rows, source).projection.ok, false)
    const missing = spawnSync(
      process.execPath,
      [script, join(dir, "absent"), "manifest"],
      { env, encoding: "utf8" },
    )
    assert.equal(missing.status, 1)
    assert.match(
      missing.stdout,
      /^::notice title=Desktop verification receipt manifest::/,
    )
    const large = join(dir, "large-result")
    writeFileSync(large, "")
    truncateSync(large, receiptMaxRawBytes * 2)
    const bounded = spawnSync(process.execPath, [script, large, "manifest"], {
      env,
      encoding: "utf8",
    })
    assert.equal(bounded.status, 1)
    const largeManifest = JSON.parse(
      Buffer.from(
        /^::notice title=Desktop verification receipt manifest::(.*)$/m.exec(
          bounded.stdout,
        )[1],
        "base64",
      ).toString("utf8"),
    )
    assert.equal(largeManifest.reason, "raw-capacity-exceeded")
    assert.equal(largeManifest.rawBytesAtLeast, receiptMaxRawBytes + 1)
    const workflow = readFileSync(join(root, ".github/workflows/local-auth.yml"), "utf8")
    const groups = [
      ...workflow.matchAll(
        /- name: Keep desktop result receipt ([123])\n\s+if: (.+)\n\s+run: (.+)/g,
      ),
    ]
    assert.equal(groups.length, receiptGroups)
    writeFileSync(join(dir, "desktop-performance.json"), JSON.stringify(example()))
    for (const group of groups) {
      assert.equal(group[2], "always() && env.NESSA_DESKTOP_PERFORMANCE == 'true'")
      // These workflow steps run only on Mac. Node entrypoint coverage above is portable.
      if (process.platform !== "win32") {
        const run = spawnSync("bash", ["-e", "-c", group[3]], {
          cwd: root,
          env,
          encoding: "utf8",
        })
        assert.equal(run.status, 0, run.stderr)
        assert.ok(
          run.stdout.trim().split("\n").filter(Boolean).length <= receiptPartsPerGroup,
        )
      }
    }
    assert.match(workflow, /cat "\$RUNNER_TEMP\/desktop-performance.json"/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
