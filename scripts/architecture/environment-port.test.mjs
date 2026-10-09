import assert from "node:assert/strict"
import test from "node:test"
import { environmentPortViolations } from "./environment-port.mjs"

const adapters = [
  "crates/nessa-server/src/conversation/infrastructure/environment.rs",
  "crates/nessa-server/src/conversation/infrastructure/ssh_environment/environment.rs",
]

test("each adapter implements the port and the service and composition name it", () => {
  for (const adapter of adapters)
    assert.deepEqual(
      environmentPortViolations(
        adapter,
        "use crate::conversation::application::{Environment, EnvironmentDeclaration, EnvironmentFuture};\nimpl Environment for SomeEnvironment {}",
      ),
      [],
    )
  assert.deepEqual(
    environmentPortViolations(
      "crates/nessa-server/src/conversation/application/service.rs",
      "environment: Arc<dyn Environment>,",
    ),
    [],
  )
  assert.deepEqual(
    environmentPortViolations(
      "crates/nessa-server/src/composition/local_auth.rs",
      "environment: crate::conversation::infrastructure::in_process_environment(),",
    ),
    [],
  )
})

test("a third adapter in product source is refused, wherever it is", () => {
  for (const path of [
    "crates/nessa-server/src/conversation/infrastructure/ssh.rs",
    "crates/nessa-server/src/conversation/infrastructure/ssh_environment/link.rs",
    "crates/nessa-server/src/env_serve/application/serve.rs",
    "crates/nessa-server/src/product/environments.rs",
  ])
    for (const source of [
      "impl Environment for Elsewhere {}",
      "impl<T: Send> Environment for Elsewhere<T> {}",
      "impl crate::conversation::application::Environment for Elsewhere {}",
    ])
      assert.equal(environmentPortViolations(path, source).length, 1, source)
})

test("a surface that names the port is refused", () => {
  for (const source of [
    "fn pick(environment: Arc<dyn Environment>) {}",
    "fn pick(environment: Arc<dyn crate::conversation::application::Environment>) {}",
    "let declared: EnvironmentDeclaration = environment.declaration();",
    "let environment = in_process_environment();",
    "use crate::conversation::application::Environment;\nfn pick<E: Environment>(e: &E) {}",
    "use crate::conversation::application::{ConversationService, Environment};",
    "use crate::conversation::application::{\n    ConversationService,\n    Environment,\n};",
    "use crate::conversation::application::*;",
    "use crate::conversation::application::environment::Environment;",
    "fn pick<E: crate::conversation::application::Environment>(e: &E) {}",
    "use crate::conversation::application;\nfn pick(e: &dyn application::Environment) {}",
    "use crate::conversation::{application::Environment, ConversationId};",
    "use super::application::Environment;",
    "use self::application::Environment;",
    "use super::super::application::{Environment, ConversationService};",
  ]) {
    assert.equal(
      environmentPortViolations("crates/nessa-server/src/product/conversation.rs", source)
        .length,
      1,
    )
    assert.equal(
      environmentPortViolations("crates/nessa-protocol/src/conversation/view.rs", source)
        .length,
      1,
    )
  }
})

test("the configuration's own Environment, a comment, and tests are not the port", () => {
  for (const source of [
    "pub fn from_environment(config: &Environment) -> Self {}",
    "use crate::config::Environment;\nimpl Default for Environment {}",
    "use crate::conversation::application::{ConversationService, ConversationError};",
    "use nessa_sdk::application::*;",
    "use nessa_sdk::application::{SessionManager, Environment};",
  ])
    assert.deepEqual(
      environmentPortViolations("crates/nessa-server/src/app/state.rs", source),
      [],
      source,
    )
  assert.deepEqual(
    environmentPortViolations(
      "crates/nessa-server/src/product/conversation.rs",
      "// never holds a dyn Environment",
    ),
    [],
  )
  assert.deepEqual(
    environmentPortViolations(
      "crates/nessa-server/tests/conversation/leases.rs",
      "impl Environment for Substitute {}",
    ),
    [],
  )
})
