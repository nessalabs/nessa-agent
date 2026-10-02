import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import {
  chmodSync,
  cpSync,
  lstatSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { test } from "node:test"

import {
  nessaUiPaths,
  sharedPackages,
  tsconfigPathViolations,
  tsconfigPaths,
  tsconfigTextViolations,
  withTsconfigPaths,
  writeTsconfigPaths,
  viteAliases,
} from "./nessa-ui-paths.mjs"

/** Each shared package's types, as `tsconfig.json` points at them. */
const sharedTypes = {
  react: ["./node_modules/@types/react"],
  "react/*": ["./node_modules/@types/react/*"],
  "react-dom": ["./node_modules/@types/react-dom"],
  "react-dom/*": ["./node_modules/@types/react-dom/*"],
}

/** Where Vite's aliases send `specifier`: the first RegExp that matches, applied with String.replace as Vite does. */
function resolveWith(aliases, specifier) {
  for (const { find, replacement } of aliases) {
    if (find.test(specifier)) return specifier.replace(find, replacement)
  }
  return null
}

test("each kind of import lands in its directory of the package's source", () => {
  const aliases = viteAliases("/ui/src")
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/app-shell"),
    "/ui/src/composites/app-shell",
  )
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/lib/size-observer"),
    "/ui/src/lib/size-observer",
  )
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/button"),
    "/ui/src/components/button",
  )
  assert.equal(
    resolveWith(aliases, "@/components/ui/button"),
    "/ui/src/components/ui/button",
  )
  assert.equal(resolveWith(aliases, "@/lib/utils"), "/ui/src/lib/utils")
  assert.equal(resolveWith(aliases, "@/provider/theme"), "/ui/src/provider/theme")
})

test("a specific rule wins over the prefix it extends, whatever order the table lists them in", () => {
  const reversed = [...nessaUiPaths].reverse()
  const aliases = viteAliases("/ui/src", reversed)
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/lib/size-observer"),
    "/ui/src/lib/size-observer",
  )
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/app-shell"),
    "/ui/src/composites/app-shell",
  )
})

test("a whole specifier is exact in Vite and TypeScript alike: a path under it is a component", () => {
  const aliases = viteAliases("/ui/src")
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/app-shell/app-shell-dock"),
    "/ui/src/components/app-shell/app-shell-dock",
  )
  // TypeScript: no `app-shell/*` key, so the same import falls to `@nessa-ui/react/*`.
  const keys = Object.keys(tsconfigPaths())
  assert.ok(keys.includes("@nessa-ui/react/app-shell"))
  assert.ok(!keys.includes("@nessa-ui/react/app-shell/*"))
  assert.ok(keys.includes("@nessa-ui/react/*"))
})

test("an import the table does not cover is left to the resolver", () => {
  const aliases = viteAliases("/ui/src")
  assert.equal(resolveWith(aliases, "@/hooks/use-thing"), null)
  assert.equal(resolveWith(aliases, "@nessa/client"), null)
  // A prefix match is on the path, not the characters: app-shell-extra is a component.
  assert.equal(
    resolveWith(aliases, "@nessa-ui/react/app-shell-extra"),
    "/ui/src/components/app-shell-extra",
  )
})

test("a specifier's punctuation is matched literally", () => {
  const [alias] = viteAliases("/ui/src", [{ specifier: "@a.b/", directory: "x/" }])
  assert.equal(alias.find.test("@aXb/y"), false)
  assert.equal(alias.find.test("@a.b/y"), true)
})

test("tsconfig's paths are the table's, then the shared types: a prefix becomes p*, a whole specifier stays whole", () => {
  assert.deepEqual(tsconfigPaths([{ specifier: "@x/", directory: "d/" }]), {
    "@x/*": ["./node_modules/@nessa-ui/react/src/d/*"],
    ...sharedTypes,
  })
  assert.deepEqual(
    tsconfigPaths([{ specifier: "@x/y", directory: "d/y", whole: true }]),
    {
      "@x/y": ["./node_modules/@nessa-ui/react/src/d/y"],
      ...sharedTypes,
    },
  )
})

test("the repository's tsconfig.json is what the table writes", () => {
  const tsconfig = JSON.parse(
    readFileSync(new URL("../tsconfig.json", import.meta.url), "utf8"),
  )
  assert.deepEqual(tsconfigPathViolations(tsconfig.compilerOptions.paths), [])
})

test("tsconfig disagreeing with the table is reported: an entry missing, or elsewhere", () => {
  const agreed = tsconfigPaths()
  const { ["@/lib/*"]: _dropped, ...missing } = agreed
  assert.match(tsconfigPathViolations(missing).join("\n"), /has no "@\/lib\/\*"/)

  const elsewhere = { ...agreed, "@nessa-ui/react/*": ["./elsewhere/*"] }
  assert.match(
    tsconfigPathViolations(elsewhere).join("\n"),
    /maps "@nessa-ui\/react\/\*" to \["\.\/elsewhere\/\*"\]/,
  )

  assert.equal(
    tsconfigPathViolations(undefined).length,
    nessaUiPaths.length + Object.keys(sharedTypes).length,
  )
  for (const violation of tsconfigPathViolations(missing))
    assert.match(violation, /run `pnpm ui:paths`/)
})

test("any entry the table does not write is reported, whatever it is", () => {
  const agreed = tsconfigPaths()
  for (const [key, targets] of [
    // A package name Vite and Vitest resolve another way (to its build).
    ["@nessa-ui/react", ["./node_modules/@nessa-ui/react/src/index.ts"]],
    // An alias that only type imports would use, which no build would load.
    ["@ui/*", ["./node_modules/@nessa-ui/react/src/components/*"]],
    // The app's own, leading nowhere near the design system.
    ["@/app/*", ["./src/app/*"]],
  ]) {
    const violations = tsconfigPathViolations({ ...agreed, [key]: targets })
    assert.equal(violations.length, 1, key)
    assert.match(
      violations[0],
      new RegExp(`has "${key.replace(/[*/]/g, "\\$&")}", which .* does not write`),
    )
  }
})

test("the shared types are required too", () => {
  const { react: _react, ...withoutReact } = tsconfigPaths()
  assert.match(tsconfigPathViolations(withoutReact).join("\n"), /has no "react"/)
})

test("the writer sets paths from the table and keeps everything else", () => {
  const before = JSON.stringify(
    {
      exclude: ["dist"],
      compilerOptions: { strict: true, paths: { "@ui/*": ["./x/*"] } },
      include: ["src"],
    },
    null,
    2,
  )
  const after = withTsconfigPaths(before)
  const written = JSON.parse(after)
  assert.deepEqual(written.exclude, ["dist"])
  assert.equal(written.compilerOptions.strict, true)
  assert.deepEqual(written.include, ["src"])
  assert.deepEqual(written.compilerOptions.paths, tsconfigPaths())
  assert.deepEqual(tsconfigTextViolations(after), [])
  assert.equal(withTsconfigPaths(after), after)
  assert.ok(after.endsWith("}\n"))
})

test("the repository's tsconfig.json is exactly what the writer makes of it", () => {
  const text = readFileSync(new URL("../tsconfig.json", import.meta.url), "utf8")
  assert.equal(withTsconfigPaths(text), text)
})

test("a key inherited from a prototype is not read as an entry", () => {
  const paths = Object.assign(Object.create({ "@/lib/*": ["./inherited/*"] }), {})
  assert.match(tsconfigPathViolations(paths).join("\n"), /has no "@\/lib\/\*"/)
})

test("tsconfig.json that is not plain JSON is reported, not thrown", () => {
  const [violation, ...rest] = tsconfigTextViolations(
    '{ "compilerOptions": { }, // a comment\n }',
  )
  assert.deepEqual(rest, [])
  assert.match(violation, /tsconfig\.json is not plain JSON/)
  assert.deepEqual(
    tsconfigTextViolations(
      JSON.stringify({ compilerOptions: { paths: tsconfigPaths() } }),
    ),
    [],
  )
})

test("each shared package, and every subpath of it, is redirected to this app's types", () => {
  assert.deepEqual(
    sharedPackages.flatMap((name) => [name, `${name}/*`]),
    Object.keys(sharedTypes),
  )
  for (const [name, targets] of Object.entries(sharedTypes))
    assert.deepEqual(tsconfigPaths()[name], targets)
})

test("baseUrl or extends, which would move the paths written, is reported", () => {
  const paths = tsconfigPaths()
  assert.match(
    tsconfigTextViolations(
      JSON.stringify({ compilerOptions: { baseUrl: "./src", paths } }),
    ).join("\n"),
    /has `baseUrl`/,
  )
  assert.match(
    tsconfigTextViolations(
      JSON.stringify({ extends: "./base.json", compilerOptions: { paths } }),
    ).join("\n"),
    /has `extends`/,
  )
})

test("a $ in the source root reaches Vite's replacement literally", () => {
  const aliases = viteAliases("/a/$&b/src")
  assert.equal(resolveWith(aliases, "@/lib/utils"), "/a/$&b/src/lib/utils")
})

test("the writer keeps CRLF line endings, and is then a no-op", () => {
  const lf = withTsconfigPaths(JSON.stringify({ compilerOptions: {} }, null, 2))
  const crlf = lf.replaceAll("\n", "\r\n")
  assert.equal(withTsconfigPaths(crlf), crlf)
  assert.equal(withTsconfigPaths(lf), lf)
})

test("the writer refuses text that is not plain JSON with a sentence", () => {
  assert.throws(
    () => withTsconfigPaths("{ // no\n }"),
    /tsconfig\.json is not plain JSON/,
  )
})

/** A copy of the writer, its table and tsconfig.json in a fresh directory whose name has a space. */
function scratchCheckout() {
  const root = mkdtempSync(join(tmpdir(), "nessa ui paths "))
  const here = (name) => fileURLToPath(new URL(name, import.meta.url))
  cpSync(here("./nessa-ui-paths.mjs"), join(root, "scripts", "nessa-ui-paths.mjs"))
  cpSync(
    here("./write-nessa-ui-paths.mjs"),
    join(root, "scripts", "write-nessa-ui-paths.mjs"),
  )
  cpSync(here("../tsconfig.json"), join(root, "tsconfig.json"))
  const run = () =>
    spawnSync(process.execPath, [join(root, "scripts", "write-nessa-ui-paths.mjs")], {
      encoding: "utf8",
      cwd: tmpdir(),
    })
  return { root, tsconfig: join(root, "tsconfig.json"), run }
}

test("the writer, run anywhere, repairs a drifted tsconfig.json and touches a current one not at all", () => {
  const { root, tsconfig, run } = scratchCheckout()
  try {
    const current = readFileSync(tsconfig, "utf8")
    const drifted = JSON.parse(current)
    drifted.compilerOptions.paths["@ui/*"] = ["./elsewhere/*"]
    writeFileSync(tsconfig, JSON.stringify(drifted, null, 2) + "\n")
    const repaired = run()
    assert.equal(repaired.status, 0, repaired.stderr)
    assert.equal(readFileSync(tsconfig, "utf8"), current)

    const written = statSync(tsconfig).mtimeMs
    const again = run()
    assert.equal(again.status, 0, again.stderr)
    assert.equal(statSync(tsconfig).mtimeMs, written)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("pnpm ui:paths, refused, says why in one line naming the file, exits 1, and leaves it as it was", () => {
  const { root, tsconfig, run } = scratchCheckout()
  try {
    const text = "{ // a comment\n}\n"
    writeFileSync(tsconfig, text)
    const refused = run()
    assert.equal(refused.status, 1)
    assert.equal(refused.stderr.trim().split("\n").length, 1, refused.stderr)
    assert.ok(
      refused.stderr.startsWith(`${tsconfig}: tsconfig.json is not plain JSON`),
      refused.stderr,
    )
    assert.equal(readFileSync(tsconfig, "utf8"), text)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("a tsconfig.json that is not an object, or whose compilerOptions is not one, is reported as such", () => {
  assert.match(tsconfigTextViolations("[]").join(""), /is not a JSON object/)
  assert.match(tsconfigTextViolations("null").join(""), /is not a JSON object/)
  assert.match(
    tsconfigTextViolations('{ "compilerOptions": [1, 2] }').join(""),
    /compilerOptions is not an object/,
  )
  assert.throws(
    () => withTsconfigPaths('{ "compilerOptions": "ab" }'),
    /compilerOptions is not an object/,
  )
})

test("a byte-order mark is accepted, and kept", () => {
  const text = "\uFEFF" + withTsconfigPaths("{}")
  assert.deepEqual(tsconfigTextViolations(text), [])
  assert.equal(withTsconfigPaths(text), text)
})

/** An in-memory file for `writeTsconfigPaths`, recording what is written. */
function memoryFile(bytes, { readError, writeError } = {}) {
  const writes = []
  return {
    writes,
    io: {
      read: () => {
        if (readError) throw readError
        return bytes
      },
      write: (_file, text) => {
        if (writeError) throw writeError
        writes.push(text)
      },
    },
  }
}

const utf8 = (text) => new TextEncoder().encode(text)
const fsError = (code) => Object.assign(new Error(`${code}: boom`), { code })

test("writeTsconfigPaths writes a drifted file once, and a current one not at all", () => {
  const current = withTsconfigPaths("{}")
  const drifted = memoryFile(utf8("{}\n"))
  assert.deepEqual(writeTsconfigPaths("t.json", drifted.io), { written: true })
  assert.deepEqual(drifted.writes, [current])
  const same = memoryFile(utf8(current))
  assert.deepEqual(writeTsconfigPaths("t.json", same.io), { written: false })
  assert.deepEqual(same.writes, [])
})

test("writeTsconfigPaths refuses in one line naming the file, whatever stops it", () => {
  for (const [why, file, pattern] of [
    [
      "missing",
      memoryFile(null, { readError: fsError("ENOENT") }),
      /^t\.json cannot be read \(ENOENT\)$/,
    ],
    [
      "a directory",
      memoryFile(null, { readError: fsError("EISDIR") }),
      /^t\.json cannot be read \(EISDIR\)$/,
    ],
    [
      "not UTF-8",
      memoryFile(Uint8Array.of(0x7b, 0x22, 0xff, 0x22, 0x7d)),
      /^t\.json is not UTF-8 text/,
    ],
    [
      "not JSON",
      memoryFile(utf8("{ // c\n}")),
      /^t\.json: tsconfig\.json is not plain JSON/,
    ],
    [
      "not an object",
      memoryFile(utf8("[]")),
      /^t\.json: tsconfig\.json is not a JSON object/,
    ],
    [
      "unwritable",
      memoryFile(utf8("{}"), { writeError: fsError("EACCES") }),
      /^t\.json cannot be written \(EACCES\); if that happened partway through, it may be partly written — look at its diff before restoring it from git, which also drops uncommitted edits$/,
    ],
  ]) {
    const result = writeTsconfigPaths("t.json", file.io)
    assert.ok("refused" in result, why)
    assert.match(result.refused, pattern, why)
    assert.deepEqual(file.writes, [], why)
  }
})

test("writeTsconfigPaths keeps a byte-order mark when it rewrites the file", () => {
  const drifted = memoryFile(utf8("\uFEFF{}\n"))
  assert.deepEqual(writeTsconfigPaths("t.json", drifted.io), { written: true })
  assert.ok(drifted.writes[0].startsWith("\uFEFF"))
})

test("a refusal is one line, even when the parser quotes the source", () => {
  const quoted = '{\n  "strict": tru,\n  "noUnusedLocals": false\n}\n'
  const { refused } = writeTsconfigPaths("t.json", memoryFile(utf8(quoted)).io)
  assert.match(refused, /^t\.json: tsconfig\.json is not plain JSON/)
  assert.equal(refused.split("\n").length, 1, refused)
  for (const thrown of [null, undefined, "x"]) {
    const result = writeTsconfigPaths("t.json", {
      read: () => {
        throw thrown
      },
      write: () => {},
    })
    assert.match(result.refused, /^t\.json cannot be read \(\S+\)$/, String(thrown))
  }
})

test("what stopped a read is one phrase, whatever was thrown", () => {
  for (const [thrown, phrase] of [
    [Object.assign(new Error("x"), { code: "E\nSPLIT  CODE" }), "E SPLIT CODE"],
    [Object.create(null), "an error with no description"],
  ]) {
    const { refused } = writeTsconfigPaths("t.json", {
      read: () => {
        throw thrown
      },
      write: () => {},
    })
    assert.equal(refused, `t.json cannot be read (${phrase})`)
  }
})

test("pnpm ui:paths writes through a symlinked tsconfig.json and keeps its mode", () => {
  const { root, tsconfig, run } = scratchCheckout()
  try {
    const current = readFileSync(tsconfig, "utf8")
    const drifted = JSON.parse(current)
    drifted.compilerOptions.paths["@ui/*"] = ["./elsewhere/*"]
    const target = join(root, "real-tsconfig.json")
    writeFileSync(target, JSON.stringify(drifted, null, 2) + "\n")
    chmodSync(target, 0o640)
    rmSync(tsconfig)
    symlinkSync(target, tsconfig)
    const result = run()
    assert.equal(result.status, 0, result.stderr)
    assert.ok(lstatSync(tsconfig).isSymbolicLink())
    assert.equal(readFileSync(target, "utf8"), current)
    assert.equal(statSync(target).mode & 0o777, 0o640)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
