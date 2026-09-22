import assert from "node:assert/strict"
import test from "node:test"
import { rustDependencyGraphViolations } from "./rust-dependency-graphs.mjs"

function metadata(edges) {
  const packages = [
    { id: "portable 0.1.0", name: "portable", version: "0.1.0" },
    { id: "bridge 0.1.0", name: "bridge", version: "0.1.0" },
    { id: "host 0.1.0", name: "host", version: "0.1.0" },
    { id: "tauri 2.0.0", name: "tauri", version: "2.0.0" },
  ]
  return {
    packages,
    workspace_members: ["portable 0.1.0", "bridge 0.1.0", "host 0.1.0"],
    resolve: {
      nodes: packages.map((pkg) => ({
        id: pkg.id,
        deps: (edges[pkg.name] ?? []).map(([name, dependency]) => ({
          name,
          pkg: packages.find((candidate) => candidate.name === dependency).id,
          dep_kinds: [{ kind: null, target: null }],
        })),
      })),
    },
  }
}

test("an unrelated desktop workspace member does not taint a selected package", () => {
  const graph = metadata({ host: [["tauri", "tauri"]] })
  assert.deepEqual(rustDependencyGraphViolations(graph, ["portable"]), [])
})

test("a direct desktop framework dependency is rejected", () => {
  const graph = metadata({ portable: [["tauri", "tauri"]] })
  assert.match(
    rustDependencyGraphViolations(graph, ["portable"])[0],
    /portable@0\.1\.0 --tauri--> tauri@2\.0\.0/,
  )
})

test("Tauri support and webview framework packages are rejected", () => {
  for (const packageName of ["tauri-plugin-dialog", "tao", "wry"]) {
    const graph = metadata({ portable: [["framework", "tauri"]] })
    graph.packages.find((pkg) => pkg.name === "tauri").name = packageName
    assert.match(
      rustDependencyGraphViolations(graph, ["portable"])[0],
      new RegExp(`desktop framework package "${packageName}"`),
    )
  }
})

test("a renamed transitive target dependency is resolved by package ID", () => {
  const graph = metadata({
    portable: [["bridge", "bridge"]],
    bridge: [["desktop_shell", "tauri"]],
  })
  graph.resolve.nodes.find(
    (node) => node.id === "bridge 0.1.0",
  ).deps[0].dep_kinds[0].target = 'cfg(target_os = "macos")'

  const violations = rustDependencyGraphViolations(graph, ["portable"])
  assert.equal(violations.length, 1)
  assert.match(
    violations[0],
    /portable@0\.1\.0 --bridge--> bridge@0\.1\.0 --desktop_shell \(renamed\)--> tauri@2\.0\.0/,
  )
})

test("a missing selected package makes the gate fail explicitly", () => {
  assert.deepEqual(rustDependencyGraphViolations(metadata({}), ["missing"]), [
    'expected exactly one workspace package named "missing", found 0',
  ])
})
