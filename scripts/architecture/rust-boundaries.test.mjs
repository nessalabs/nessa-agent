import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import test from "node:test"
import {
  maskRustNonCode,
  normalizedPath,
  rustBoundaryViolations,
  workspaceRustSourceRoots,
} from "./rust-boundaries.mjs"

test("domain rejects outward imports, grouped effects and runtime dependencies", () => {
  for (const source of [
    "use crate::application::ConversationView;",
    "use crate::{domain::Id, infrastructure::Repository};",
    "use tokio::sync::Mutex;",
    "use std::{path::PathBuf, fs::File};",
    "use serde::Serialize;",
  ]) {
    assert.ok(rustBoundaryViolations("src/domain/value.rs", source).length)
    assert.ok(rustBoundaryViolations("src/shell/domain.rs", source).length)
  }
})

test("qualified references obey the same layer rules as imports", () => {
  for (const source of [
    'fn read() { std::fs::read("file"); }',
    'fn read() { ::std /* boundary */ :: fs :: read("file"); }',
    'fn read() { std::r#fs::read("file"); }',
    'fn run() { std::process::Command::new("tool"); }',
    'fn connect() { std::net::TcpStream::connect("host"); }',
    "type Lock = tokio::sync::Mutex<Value>;",
    "#[derive(serde::Serialize)] struct Value;",
    "fn serialize() { serde_json::json!({}); }",
    "fn call() { reqwest::get(url); }",
    "fn open() { crate::infrastructure::Repository::new(); }",
    "fn open() { crate::领域::infrastructure::仓库::new(); }",
    "type Value = crate::领域::application::值;",
    "fn open() { <Value as crate::application::Port>::open(); }",
    "type Source = nessa_sync::replication::catalogue::Source;",
  ]) {
    assert.ok(rustBoundaryViolations("src/domain/value.rs", source).length, source)
    assert.ok(rustBoundaryViolations("src/shell/domain.rs", source).length, source)
  }
  for (const layer of [
    "infrastructure",
    "composition",
    "product",
    "adapters",
    "presentation",
  ]) {
    const source = `fn open() { crate::${layer}::Repository::new(); }`
    assert.ok(rustBoundaryViolations("src/application/service.rs", source).length, source)
  }
  assert.ok(
    rustBoundaryViolations(
      "src/application/service.rs",
      "fn open() { crate::领域::infrastructure::仓库::new(); }",
    ).length,
  )
})

test("qualified inward values and application runtime coordination remain allowed", () => {
  for (const source of [
    "type Moment = std::time::Duration;",
    "type Id = crate::domain::Identity;",
    "type Id = crate::领域::domain::标识;",
    "type Id = crate::domain::infrastructure名::标识;",
    "type Id = crate::domain::tokio名::标识;",
    "type Id = nessa_sync::replication::domain::Id;",
    "use std::net::SocketAddr as Address;",
    "use std::net::{IpAddr as Address, SocketAddr};",
    "fn address() { std :: net :: SocketAddr::new(ip, port); }",
    "fn address() { ::std::net::Ipv4Addr::LOCALHOST; }",
  ])
    assert.deepEqual(rustBoundaryViolations("src/domain/value.rs", source), [], source)
  assert.deepEqual(
    rustBoundaryViolations(
      "src/application/service.rs",
      "fn run() { tokio::spawn(work); }",
    ),
    [],
  )
  for (const layer of ["infrastructure", "composition"]) {
    assert.deepEqual(
      rustBoundaryViolations(
        `src/${layer}/service.rs`,
        'fn read() { std::fs::read("file"); }',
      ),
      [],
    )
  }
})

test("literal and comment text does not create or hide qualified references", () => {
  const literal = String.raw`
    // std::fs::read("file");
    /* outer crate::infrastructure::Repository::new(); /* serde::Serialize */ */
    const TEXT: &str = "serde_json::json! and std::fs::read";
    const RAW: &str = br###"tokio::spawn and std::process::Command"###;
    const CHAR: char = '"';
    fn valid<'a>(value: &'a str) { let symbol = '😀'; }
  `
  assert.deepEqual(rustBoundaryViolations("src/domain/value.rs", literal), [])
  assert.ok(
    rustBoundaryViolations(
      "src/domain/value.rs",
      `${literal}\nfn read() { std::fs::read("file"); }`,
    ).length,
  )
  assert.ok(
    rustBoundaryViolations(
      "src/domain/value.rs",
      'const TEXT: &str = "😀"; fn read() { std::fs::read("file"); }',
    ).length,
  )
  const unicode = 'const TEXT: &str = "😀"; fn read() { std::fs::read("file"); }'
  const masked = maskRustNonCode(unicode)
  assert.equal(masked.length, unicode.length)
  assert.equal(masked.indexOf("fn read"), unicode.indexOf("fn read"))
})

test("forbidden references fail in bare Node without installed packages", () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-rust-boundaries-"))
  try {
    const module = join(directory, "rust-boundaries.mjs")
    copyFileSync(new URL("./rust-boundaries.mjs", import.meta.url), module)
    const script = `
      import { rustBoundaryViolations } from ${JSON.stringify(pathToFileURL(module).href)};
      const violations = rustBoundaryViolations("src/domain/value.rs", process.argv[1]);
      console.log(JSON.stringify(violations));
      process.exitCode = violations.length ? 1 : 0;
    `
    for (const [source, status] of [
      ["use std::fs::read;", 1],
      ['fn read() { std::fs::read("file"); }', 1],
      ["#[derive(serde::Serialize)] struct Value;", 1],
      ["type Moment = std::time::Duration;", 0],
    ]) {
      const result = spawnSync(
        process.execPath,
        ["--input-type=module", "--eval", script, source],
        {
          cwd: directory,
          encoding: "utf8",
          timeout: 10000,
        },
      )
      assert.ifError(result.error)
      assert.equal(result.status, status, result.stderr || source)
      assert.equal(result.stderr, "")
      assert.equal(JSON.parse(result.stdout).length > 0, status === 1)
    }
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("domain may use the sync engine's domain values and nothing else of it", () => {
  assert.deepEqual(
    rustBoundaryViolations(
      "src/domain/value.rs",
      "use nessa_sync::replication::domain::{Id, Scope};",
    ),
    [],
  )
  for (const source of [
    "use nessa_sync::replication::catalogue::CatalogueSourceError;",
    "use nessa_sync::{replication::domain::Id, transport::Client};",
  ]) {
    assert.ok(rustBoundaryViolations("src/domain/value.rs", source).length, source)
  }
})

test("application may coordinate async ports but may not construct adapters", () => {
  assert.deepEqual(
    rustBoundaryViolations(
      "src/application/service.rs",
      `
    use crate::domain::Id;
    use tokio::sync::Mutex;
    use super::ports::Repository;
  `,
    ),
    [],
  )
  assert.ok(
    rustBoundaryViolations(
      "src/shell/application.rs",
      "use crate::shell::infrastructure::ShepherdRunner;",
    ).length,
  )
})

test("documentation and inward imports do not trigger failures", () => {
  assert.deepEqual(
    rustBoundaryViolations(
      "src/domain/id.rs",
      `
    //! use crate::application::Agent;
    /* use tokio::sync::Mutex; */
    use std::{fmt, time::Duration};
    use super::Identity;
  `,
    ),
    [],
  )
  assert.deepEqual(
    rustBoundaryViolations(
      "src/composition/root.rs",
      "use crate::infrastructure::Repository;",
    ),
    [],
  )
})

test("strings, raw strings, and nested block comments cannot forge Rust imports", () => {
  const source = String.raw`
    const NORMAL: &str = "use tokio::sync::Mutex; /*";
    const RAW: &str = r###"use crate::infrastructure::Store; //"###;
    /* outer use serde::Serialize;
       /* nested use crate::application::Command; */
    */
    use super::Identity;
  `
  assert.deepEqual(rustBoundaryViolations("src/domain/value.rs", source), [])
  assert.equal(maskRustNonCode(source).includes("use super::Identity;"), true)
})

test("Windows separators normalize before boundary classification", () => {
  assert.equal(normalizedPath("src\\domain\\value.rs"), "src/domain/value.rs")
  assert.ok(
    rustBoundaryViolations(
      "crate\\src\\domain\\value.rs",
      "use crate::application::Command;",
    ).length,
  )
})

test("scanner discovers every Cargo workspace package and Tauri Rust root", () => {
  const root = resolve(import.meta.dirname, "../..")
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: root,
      encoding: "utf8",
    }),
  )
  const roots = workspaceRustSourceRoots(root, metadata).map(normalizedPath)
  for (const pkg of metadata.packages.filter((pkg) =>
    metadata.workspace_members.includes(pkg.id),
  ))
    assert.ok(roots.includes(normalizedPath(resolve(pkg.manifest_path, ".."))))
  assert.ok(roots.some((path) => path.endsWith("/src-tauri")))
})

test("standalone Tauri discovery excludes local agent worktree copies", () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-rust-roots-"))
  try {
    for (const path of ["desktop/src-tauri", ".claude/worktrees/other/src-tauri"]) {
      mkdirSync(join(directory, path), { recursive: true })
      writeFileSync(join(directory, path, "Cargo.toml"), "")
    }
    assert.deepEqual(
      workspaceRustSourceRoots(directory, { workspace_members: [], packages: [] }),
      [join(directory, "desktop/src-tauri")],
    )
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test("domain may import pure address values but no net module or socket effects", () => {
  for (const value of [
    "IpAddr",
    "Ipv4Addr",
    "Ipv6Addr",
    "SocketAddr",
    "SocketAddrV4",
    "SocketAddrV6",
  ]) {
    assert.deepEqual(
      rustBoundaryViolations("src/domain/endpoint.rs", `use std::net::${value};`),
      [],
    )
  }
  assert.deepEqual(
    rustBoundaryViolations(
      "src/domain/endpoint.rs",
      "use std::net::{IpAddr, SocketAddr};",
    ),
    [],
  )
  for (const source of [
    "use std::net;",
    "use std::net::*;",
    "use std::net::TcpStream;",
    "use std::net::TcpListener;",
    "use std::net::{SocketAddr, TcpStream};",
    "use std::{net, fs::File};",
  ]) {
    assert.ok(rustBoundaryViolations("src/domain/endpoint.rs", source).length, source)
  }
})
