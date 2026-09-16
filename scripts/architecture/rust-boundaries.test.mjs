import assert from "node:assert/strict"
import test from "node:test"
import { rustBoundaryViolations } from "./rust-boundaries.mjs"

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
