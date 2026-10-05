import assert from "node:assert/strict"
import test from "node:test"
import {
  PACKAGE_DENYLISTS,
  PORTABLE_RUST_PACKAGES,
  rustDependencyGraphViolations,
} from "./rust-dependency-graphs.mjs"

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

test("a package on the selected crate's denylist is rejected, transitively", () => {
  const graph = metadata({
    portable: [["bridge", "bridge"]],
    bridge: [["host", "host"]],
  })
  assert.deepEqual(rustDependencyGraphViolations(graph, ["portable"], {}), [])
  assert.match(
    rustDependencyGraphViolations(graph, ["portable"], { portable: ["host"] })[0],
    /"portable" reaches denied package "host" through portable@0\.1\.0 --bridge--> bridge@0\.1\.0 --host--> host@0\.1\.0/,
  )
  assert.deepEqual(
    rustDependencyGraphViolations(graph, ["bridge"], { portable: ["host"] }),
    [],
  )
})

test("the device client stays portable and cannot reach the gateway, including renamed transitive edges", () => {
  assert.ok(PORTABLE_RUST_PACKAGES.includes("nessa-client-core"))
  const graph = metadata({
    portable: [["bridge", "bridge"]],
    bridge: [["gateway", "host"]],
  })
  graph.packages.find((pkg) => pkg.name === "portable").name = "nessa-client-core"
  graph.packages.find((pkg) => pkg.name === "host").name = "nessa-server"
  assert.deepEqual(rustDependencyGraphViolations(graph, ["nessa-client-core"], {}), [])
  assert.match(
    rustDependencyGraphViolations(graph, ["nessa-client-core"], PACKAGE_DENYLISTS)[0],
    /reaches denied package "nessa-server"/,
  )
})

test("the gateway consumes the device client only through dev edges, even when renamed", () => {
  const graph = metadata({ portable: [["phone", "bridge"]] })
  graph.packages.find((pkg) => pkg.name === "portable").name = "nessa-server"
  graph.packages.find((pkg) => pkg.name === "bridge").name = "nessa-client-core"
  const edge = graph.resolve.nodes.find((node) => node.id === "portable 0.1.0").deps[0]
  edge.dep_kinds[0].kind = "dev"
  assert.deepEqual(rustDependencyGraphViolations(graph, ["nessa-server"]), [])
  for (const kinds of [[null], ["build"], ["dev", null], ["dev", "build"]]) {
    edge.dep_kinds = kinds.map((kind) => ({ kind, target: null }))
    assert.match(
      rustDependencyGraphViolations(graph, ["nessa-server"])[0],
      /dev-only package "nessa-client-core" through a non-dev dependency path .*phone/,
    )
  }
})

test("gateway normal/build paths cannot reach the client transitively, while downstream dev edges are excluded", () => {
  const graph = metadata({
    portable: [["middle", "bridge"]],
    bridge: [["phone", "host"]],
  })
  graph.packages.find((pkg) => pkg.name === "portable").name = "nessa-server"
  graph.packages.find((pkg) => pkg.name === "host").name = "nessa-client-core"
  const rootEdge = graph.resolve.nodes.find((node) => node.id === "portable 0.1.0")
    .deps[0]
  const nextEdge = graph.resolve.nodes.find((node) => node.id === "bridge 0.1.0").deps[0]
  for (const kinds of [[null], ["build"], ["dev", null], ["dev", "build"]]) {
    rootEdge.dep_kinds = kinds.map((kind) => ({ kind, target: null }))
    assert.match(
      rustDependencyGraphViolations(graph, ["nessa-server"])[0],
      /non-dev dependency path .*middle.*phone/,
    )
  }
  rootEdge.dep_kinds = [{ kind: "dev", target: null }]
  assert.deepEqual(rustDependencyGraphViolations(graph, ["nessa-server"]), [])
  rootEdge.dep_kinds = [{ kind: null, target: null }]
  nextEdge.dep_kinds = [{ kind: "dev", target: null }]
  assert.deepEqual(rustDependencyGraphViolations(graph, ["nessa-server"]), [])
  nextEdge.dep_kinds = [
    { kind: "dev", target: null },
    { kind: null, target: null },
  ]
  assert.equal(rustDependencyGraphViolations(graph, ["nessa-server"]).length, 1)
})

test("a first dev path cannot mask a later production path through the same package", () => {
  const graph = metadata({
    portable: [
      ["test_helper", "bridge"],
      ["runtime", "host"],
    ],
    host: [["shared", "bridge"]],
    bridge: [["phone", "tauri"]],
  })
  graph.packages.find((pkg) => pkg.name === "portable").name = "nessa-server"
  graph.packages.find((pkg) => pkg.name === "tauri").name = "nessa-client-core"
  graph.resolve.nodes.find(
    (node) => node.id === "portable 0.1.0",
  ).deps[0].dep_kinds[0].kind = "dev"
  assert.match(
    rustDependencyGraphViolations(graph, ["nessa-server"])[0],
    /non-dev dependency path .*runtime.*shared.*phone/,
  )
})
