import assert from "node:assert/strict"
import test from "node:test"
import { environmentPortViolations } from "./environment-port.mjs"

const adapter = "crates/nessa-server/src/conversation/infrastructure/environment.rs"

test("the in-process adapter implements the port and the service and composition name it", () => {
  assert.deepEqual(
    environmentPortViolations(adapter, "impl Environment for InProcessEnvironment {}"),
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

test("a second adapter in product source is refused, wherever it is", () => {
  for (const path of [
    "crates/nessa-server/src/conversation/infrastructure/ssh.rs",
    "crates/nessa-server/src/product/environments.rs",
  ])
    assert.equal(
      environmentPortViolations(path, "impl Environment for Elsewhere {}").length,
      1,
    )
})

test("a surface that names the port is refused", () => {
  for (const source of [
    "fn pick(environment: Arc<dyn Environment>) {}",
    "let declared: EnvironmentDeclaration = environment.declaration();",
    "let environment = in_process_environment();",
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
  assert.deepEqual(
    environmentPortViolations(
      "crates/nessa-server/src/app/state.rs",
      "pub fn from_environment(config: &Environment) -> Self {}",
    ),
    [],
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
