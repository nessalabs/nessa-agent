import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { test } from "node:test"

import {
  nessaUiPaths,
  sharedPackages,
  tsconfigPathViolations,
  tsconfigPaths,
  tsconfigTextViolations,
  withTsconfigPaths,
  viteAliases,
} from "./nessa-ui-paths.mjs"

/** Each shared package's types, as `tsconfig.json` points at them. */
const sharedTypes = {
  react: ["./node_modules/@types/react"],
  "react-dom": ["./node_modules/@types/react-dom"],
}

/** Where Vite's aliases send `specifier`: the first that matches, as Vite applies them. */
function resolveWith(aliases, specifier) {
  for (const { find, replacement } of aliases) {
    if (typeof find === "string") {
      if (specifier === find || specifier.startsWith(`${find}/`))
        return replacement + specifier.slice(find.length)
    } else if (find.test(specifier)) {
      return specifier.replace(find, replacement)
    }
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

test("a key inherited from Object.prototype is not read as an entry", () => {
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

test("the shared packages are the ones whose types tsconfig redirects", () => {
  assert.deepEqual(sharedPackages, Object.keys(sharedTypes))
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

test("pnpm ui:paths leaves the repository's tsconfig.json as it is", () => {
  const file = new URL("../tsconfig.json", import.meta.url)
  const before = readFileSync(file, "utf8")
  const run = spawnSync(
    process.execPath,
    [new URL("./write-nessa-ui-paths.mjs", import.meta.url).pathname],
    { encoding: "utf8" },
  )
  assert.equal(run.status, 0, run.stderr)
  assert.equal(readFileSync(file, "utf8"), before)
})
