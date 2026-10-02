import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"

import {
  nessaUiPaths,
  tsconfigPathViolations,
  tsconfigPaths,
  viteAliases,
} from "./nessa-ui-paths.mjs"

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

test("tsconfig's paths are the table's: a prefix becomes p*, a whole specifier stays whole", () => {
  assert.deepEqual(tsconfigPaths([{ specifier: "@x/", directory: "d/" }]), {
    "@x/*": ["./node_modules/@nessa-ui/react/src/d/*"],
  })
  assert.deepEqual(
    tsconfigPaths([{ specifier: "@x/y", directory: "d/y", whole: true }]),
    {
      "@x/y": ["./node_modules/@nessa-ui/react/src/d/y"],
    },
  )
})

test("the repository's tsconfig.json agrees with the table", () => {
  const tsconfig = JSON.parse(
    readFileSync(new URL("../tsconfig.json", import.meta.url), "utf8"),
  )
  assert.deepEqual(tsconfigPathViolations(tsconfig.compilerOptions.paths), [])
})

test("tsconfig disagreeing with the table is reported: missing, elsewhere, or not the table's", () => {
  const agreed = tsconfigPaths()
  const { ["@/lib/*"]: _dropped, ...missing } = agreed
  assert.match(tsconfigPathViolations(missing).join("\n"), /has no "@\/lib\/\*"/)

  const elsewhere = { ...agreed, "@nessa-ui/react/*": ["./elsewhere/*"] }
  assert.match(
    tsconfigPathViolations(elsewhere).join("\n"),
    /maps "@nessa-ui\/react\/\*" to \["\.\/elsewhere\/\*"\]/,
  )

  const extra = { ...agreed, "@/hooks/*": ["./node_modules/@nessa-ui/react/src/hooks/*"] }
  assert.match(
    tsconfigPathViolations(extra).join("\n"),
    /has "@\/hooks\/\*", which .* does not map/,
  )

  assert.equal(tsconfigPathViolations(undefined).length, nessaUiPaths.length)
})

test("entries that are not the design system's are left alone", () => {
  const withOthers = { ...tsconfigPaths(), react: ["./node_modules/@types/react"] }
  assert.deepEqual(tsconfigPathViolations(withOthers), [])
})

test("a key inherited from Object.prototype is not read as an entry", () => {
  const paths = Object.assign(Object.create({ "@/lib/*": ["./inherited/*"] }), {})
  assert.match(tsconfigPathViolations(paths).join("\n"), /has no "@\/lib\/\*"/)
})
