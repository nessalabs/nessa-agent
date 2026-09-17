import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { resolve } from "node:path"
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
