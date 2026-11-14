import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { spawnSync } from "node:child_process"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { spawnProcess, withFailureAnnotations } from "./process-failure.mjs"

function message(line) {
  assert.equal(line.split("\n").length, 2, "one workflow command line")
  assert.match(line, /^::error title=Check process failed::/)
  return decodeURIComponent(line.slice(line.indexOf("::", 2) + 2, -1))
}

function assemble(lines) {
  const parts = lines.map((line) => {
    const text = message(line).slice(0, 4096)
    const match = /^Retained report ([a-f0-9]{64}) part (\d+)\/(\d+)\n/.exec(text)
    assert.ok(match, "each received part identifies its retained report")
    return {
      hash: match[1],
      index: Number(match[2]),
      count: Number(match[3]),
      data: text.slice(match[0].length),
    }
  })
  assert.ok(parts.length > 0)
  assert.equal(parts.length, parts[0].count, "all retained report parts arrived")
  assert.ok(
    parts.every(
      (part) =>
        part.hash === parts[0].hash &&
        part.count === parts.length &&
        part.index >= 1 &&
        part.index <= parts.length,
    ),
    "one complete retained report",
  )
  assert.equal(
    new Set(parts.map((part) => part.index)).size,
    parts.length,
    "no duplicate part",
  )
  const report = parts
    .sort((a, b) => a.index - b.index)
    .map((part) => part.data)
    .join("")
  assert.equal(createHash("sha256").update(report).digest("hex"), parts[0].hash)
  return report
}

test("failure annotation retains split stdout/stderr, exact invocation and exit", async () => {
  const original = { cwd: "/synthetic", marker: Symbol("unchanged") }
  const forwarded = []
  const commands = []
  const args = ["--synthetic", "space argument"]
  const wrapped = withFailureAnnotations(
    async (command, receivedArgs, io) => {
      assert.equal(command, "fixture.exe")
      assert.equal(receivedArgs, args)
      assert.equal(io.cwd, original.cwd)
      assert.equal(io.marker, original.marker)
      io.onStdout("test synthetic::fails ... FAI")
      io.onStderr("LED\nthread 'synthetic::fails' panicked\n")
      io.onStderr("assertion `left == right` failed\n")
      return 101
    },
    (line) => commands.push(line),
  )
  const code = await wrapped("fixture.exe", args, {
    ...original,
    onStdout: (chunk) => forwarded.push(["out", chunk]),
    onStderr: (chunk) => forwarded.push(["err", chunk]),
  })
  assert.equal(code, 101)
  assert.equal(commands.length, 1)
  assert.match(
    message(commands[0]),
    /fixture\.exe \["--synthetic","space argument"\] exited with 101/,
  )
  assert.match(
    message(commands[0]),
    /FAILED\nthread 'synthetic::fails' panicked\nassertion `left == right` failed/,
  )
  assert.deepEqual(
    forwarded.map(([stream]) => stream),
    ["out", "err", "err"],
  )
  assert.equal(forwarded[0][1], "test synthetic::fails ... FAI")
})

test("output cannot inject workflow commands; percent and CR/LF survive decoding", async () => {
  const commands = []
  const text = "100%\r\n::error title=Injected::payload%0Aend"
  await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStdout(text)
      return 7
    },
    (line) => commands.push(line),
  )("fixture", [], {})
  assert.equal(commands.length, 1)
  assert.match(commands[0], /100%25%0D%0A::error/)
  assert.ok(message(commands[0]).endsWith(text))
})

test("bounded output names truncation, keeps the final assertion and forwards all output", async () => {
  const annotations = []
  let forwarded = ""
  const output =
    "discarded-prefix" + "x".repeat(40_000) + "\nassertion failed: synthetic-tail"
  await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStdout(output.slice(0, 100))
      io.onStdout(output.slice(100))
      return 2
    },
    (line) => annotations.push(line),
  )("fixture", [], {
    onStdout: (chunk) => {
      forwarded += chunk
    },
  })
  assert.equal(forwarded, output)
  const text = assemble(annotations)
  assert.match(text, /Output truncated; last 16384 characters:/)
  assert.doesNotMatch(text, /discarded-prefix/)
  assert.ok(text.endsWith(output.slice(-16_384)))
  assert.ok(text.length < 16_600)
})

test("a successful child is not reclassified by failure-shaped output", async () => {
  let annotations = 0
  const result = await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStderr("FAILED\nthread panicked\n::error::synthetic\n")
      return 0
    },
    () => {
      annotations += 1
    },
  )("fixture", [], {})
  assert.equal(result, 0)
  assert.equal(annotations, 0)
})

test("spawn rejection preserves the same error and annotates its output", async () => {
  const failure = new Error("synthetic spawn refusal")
  let annotation = ""
  const wrapped = withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStderr("synthetic diagnostic")
      throw failure
    },
    (line) => {
      annotation = line
    },
  )
  await assert.rejects(wrapped("fixture", [], {}), (error) => error === failure)
  assert.match(message(annotation), /failed: synthetic spawn refusal/)
  assert.match(message(annotation), /synthetic diagnostic/)
})

test("a 4096-character receiver retains the complete bounded failure tail", async () => {
  const annotations = []
  const output =
    "discarded-prefix" + "x".repeat(40_000) + "\nassertion failed: receiver-tail"
  await withFailureAnnotations(
    async (_command, _args, io) => {
      io.onStdout(output)
      return 101
    },
    (line) => annotations.push(line),
  )("fixture", [], {})
  const decoded = annotations.map(message)
  assert.ok(
    decoded.every((text) => text.length <= 3500),
    "each complete decoded annotation survives the receiver bound",
  )
  const received = decoded.map((text) => text.slice(0, 4096))
  assert.match(
    received[0],
    new RegExp(`part ${annotations.length}/${annotations.length}\\n`),
    "the final failure section has priority",
  )
  const report = assemble(annotations)
  assert.ok(report.endsWith(output.slice(-16_384)), "the complete retained tail survives")
  assert.ok(received[0].includes("assertion failed: receiver-tail"))
})

test("long invocation and rejection previews stay bounded without crowding out the tail", async () => {
  const annotations = []
  const output = "🙂%\r\n::error::literal".repeat(1200)
  const failure = new Error("rejected " + "🙂".repeat(20_000))
  const args = ["argument".repeat(10_000)]
  const command = "path".repeat(10_000)
  const code = withFailureAnnotations(
    async (receivedCommand, receivedArgs, io) => {
      assert.equal(receivedCommand, command)
      assert.equal(receivedArgs, args)
      io.onStderr(output)
      throw failure
    },
    (line) => annotations.push(line),
  )
  await assert.rejects(code(command, args, {}), (error) => error === failure)
  assert.ok(annotations.length <= 6)
  for (const line of annotations) assert.ok(message(line).length <= 3500)
  const report = assemble(annotations)
  assert.match(report, /failed: rejected/)
  assert.equal((report.match(/preview truncated;/g) ?? []).length, 2)
  assert.ok(
    report.includes(
      createHash("sha256")
        .update(`${command} ${JSON.stringify(args)}`)
        .digest("hex"),
    ),
  )
  assert.ok(report.includes(createHash("sha256").update(failure.message).digest("hex")))
  assert.ok(report.endsWith(output.slice(-16_384)))
})

test("Unicode chunk boundaries and workflow escapes round trip without changing the captured text", async () => {
  for (const text of [
    "🙂".repeat(2500) + "Z",
    "x".repeat(3205) + "🙂%0A\r\n::error title=Injected::payload".repeat(200),
  ]) {
    const annotations = []
    await withFailureAnnotations(
      async (_command, _args, io) => {
        io.onStdout(text)
        return 3
      },
      (line) => annotations.push(line),
    )("fixture", [], {})
    assert.ok(annotations.length > 1)
    for (const line of annotations) {
      const decoded = message(line)
      assert.ok(decoded.length <= 3500)
      assert.ok(decoded.isWellFormed())
    }
    assert.ok(assemble(annotations).endsWith(text))
  }
})

test("incomplete, duplicate, conflicting and receiver-masked parts are not complete reports", async () => {
  const collect = async (ending) => {
    const lines = []
    await withFailureAnnotations(
      async (_command, _args, io) => {
        io.onStdout("x".repeat(10_000) + ending)
        return 101
      },
      (line) => lines.push(line),
    )("fixture", [], {})
    return lines
  }
  const first = await collect("first")
  const second = await collect("second")
  assert.throws(() => assemble(first.slice(1)))
  assert.throws(() => assemble([first[0], ...first.slice(0, -1)]))
  assert.throws(() => assemble([second[0], ...first.slice(1)]))
  assert.throws(() => assemble(first.map((line) => line.replace("first", "***"))))
  // Content correlation is not an execution identity: identical reports cannot
  // prove that two separate process executions occurred.
  assert.throws(() => assemble([...first, ...first]))
})

test("a reporting failure cannot replace the failed child's result or rejection", async () => {
  const write = () => {
    throw new Error("reporting writer failed")
  }
  assert.equal(
    await withFailureAnnotations(async () => 101, write)("fixture", [], {}),
    101,
  )
  const failure = new Error("original rejection")
  await assert.rejects(
    withFailureAnnotations(async () => {
      throw failure
    }, write)("fixture", [], {}),
    (error) => error === failure,
  )
  const getterFailure = Object.defineProperty({}, "message", {
    get() {
      throw new Error("message getter failed")
    },
  })
  await assert.rejects(
    withFailureAnnotations(
      async () => {
        throw getterFailure
      },
      () => {},
    )("fixture", [], {}),
    (error) => error === getterFailure,
  )
})

const cli = fileURLToPath(new URL("./process-failure.mjs", import.meta.url))

function fixture(action) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "nessa process failure-")))
  try {
    return action(root)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

for (const code of [0, 1, 2, 101])
  test(`actual CLI preserves child exit ${code}, argv, cwd, environment and streams`, () => {
    fixture((root) => {
      const child = join(root, "child.mjs")
      writeFileSync(
        child,
        `console.log(JSON.stringify({args:process.argv.slice(2),cwd:process.cwd(),marker:process.env.NESSA_PROCESS_FAILURE_FIXTURE}));console.error('child stderr: 100%');process.exitCode=${code};\n`,
      )
      const args = ["space argument", "literal $(echo inert)", "🙂", "--flag"]
      const result = spawnSync(
        process.execPath,
        [cli, process.execPath, child, ...args],
        {
          cwd: root,
          env: {
            ...process.env,
            GITHUB_ACTIONS: "true",
            NESSA_PROCESS_FAILURE_FIXTURE: "inherited without changing the child",
          },
          encoding: "utf8",
          timeout: 30_000,
        },
      )
      assert.equal(result.error, undefined)
      assert.equal(result.status, code, result.stderr)
      assert.deepEqual(JSON.parse(result.stdout), {
        args,
        cwd: root,
        marker: "inherited without changing the child",
      })
      assert.ok(result.stderr.startsWith("child stderr: 100%\n"))
      const annotations = result.stderr
        .split("\n")
        .filter((line) => line.startsWith("::error title=Check process failed::"))
        .map((line) => line + "\n")
      if (code === 0) assert.deepEqual(annotations, [])
      else {
        const report = assemble(annotations)
        assert.ok(report.includes(`exited with ${code}`))
        assert.ok(report.includes("child stderr: 100%"))
        assert.ok(report.includes(JSON.stringify(args)))
      }
    })
  })

test("local CLI forwards a failure without producing CI annotations", () => {
  const result = spawnSync(
    process.execPath,
    [cli, process.execPath, "-e", "console.error('local failure');process.exitCode=2"],
    {
      env: { ...process.env, GITHUB_ACTIONS: "false" },
      encoding: "utf8",
      timeout: 30_000,
    },
  )
  assert.equal(result.error, undefined)
  assert.equal(result.status, 2)
  assert.equal(result.stdout, "")
  assert.equal(result.stderr, "local failure\n")
})

test("CLI spawn rejection remains failure and missing executable prints usage", () => {
  fixture((root) => {
    const command = join(root, "executable-that-does-not-exist")
    const result = spawnSync(process.execPath, [cli, command], {
      env: { ...process.env, GITHUB_ACTIONS: "true" },
      encoding: "utf8",
      timeout: 30_000,
    })
    assert.equal(result.error, undefined)
    assert.equal(result.status, 1)
    assert.equal(result.stdout, "")
    assert.match(result.stderr, /::error title=Check process failed::/)
    assert.ok(result.stderr.includes(command))
    const missing = spawnSync(process.execPath, [cli], {
      env: { ...process.env, GITHUB_ACTIONS: "true" },
      encoding: "utf8",
      timeout: 30_000,
    })
    assert.equal(missing.error, undefined)
    assert.equal(missing.status, 1)
    assert.equal(missing.stdout, "")
    assert.equal(
      missing.stderr,
      "usage: process-failure.mjs <executable> [arguments...]\n",
    )
  })
})

test("the real adapter waits for inherited streams to close after the direct child exits", async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "nessa process close-")))
  try {
    const grandchild = join(root, "late.mjs"),
      child = join(root, "child.mjs")
    writeFileSync(
      grandchild,
      "setTimeout(()=>{process.stdout.write('late stdout');process.stderr.write('late stderr')},50)\n",
    )
    writeFileSync(
      child,
      // Windows libuv otherwise kills the writer with this direct child's job
      // (Node v24.19.0, deps/uv/src/win/process.c, UV_PROCESS_DETACHED).
      "import{spawn}from'node:child_process';const child=spawn(process.execPath,[process.argv[2]],{detached:true,stdio:['ignore',process.stdout,process.stderr]});child.unref();process.exitCode=7\n",
    )
    let stdout = "",
      stderr = ""
    const code = await spawnProcess(process.execPath, [child, grandchild], {
      cwd: root,
      onStdout: (chunk) => {
        stdout += chunk
      },
      onStderr: (chunk) => {
        stderr += chunk
      },
    })
    assert.equal(code, 7)
    assert.equal(stdout, "late stdout")
    assert.equal(stderr, "late stderr")
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("the existing desktop workflow wraps its unchanged checker and runs portable process tests", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  assert.equal(
    workflow.split("run: node scripts/process-failure.mjs node scripts/check-desktop.mjs")
      .length - 1,
    1,
  )
  assert.equal(workflow.includes("run: node scripts/check-desktop.mjs"), false)
  assert.match(
    workflow,
    /^ {6}- run: node scripts\/process-failure\.mjs node scripts\/check-desktop\.mjs\r?$/m,
  )
  const desktop = workflow
    .split("run: node scripts/process-failure.mjs node scripts/check-desktop.mjs")[1]
    ?.split("\n      - ")[0]
  assert.ok(desktop !== undefined)
  assert.doesNotMatch(desktop, /continue-on-error|if:/)
  const checker = readFileSync(new URL("./check-desktop.mjs", import.meta.url), "utf8")
  assert.ok(checker.includes('"scripts/process-failure.test.mjs"'))
})

test("actual CLI bounds and escapes the failed tail while forwarding complete output", () => {
  fixture((root) => {
    const child = join(root, "large failure.mjs")
    const output =
      "discarded-prefix" +
      "x".repeat(50_000) +
      "\n100%\r\n::error title=Injected::payload%0A\nassertion failed: actual retained tail"
    writeFileSync(
      child,
      `process.stdout.write(${JSON.stringify(output)});process.exitCode=101;\n`,
    )
    const result = spawnSync(process.execPath, [cli, process.execPath, child], {
      env: { ...process.env, GITHUB_ACTIONS: "true" },
      encoding: "utf8",
      timeout: 30_000,
    })
    assert.equal(result.error, undefined)
    assert.equal(result.status, 101)
    assert.equal(result.stdout, output)
    const annotations = result.stderr
      .trimEnd()
      .split("\n")
      .map((line) => line + "\n")
    assert.ok(annotations.every((line) => line.length < 3500))
    const report = assemble(annotations)
    assert.match(report, /Output truncated; last 16384 characters:/)
    assert.doesNotMatch(report, /discarded-prefix/)
    assert.ok(report.includes("100%\r\n::error title=Injected::payload%0A"))
    assert.ok(report.endsWith(output.slice(-16_384)))
  })
})
