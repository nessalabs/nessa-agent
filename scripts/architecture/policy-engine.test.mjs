import assert from "node:assert/strict"
import test from "node:test"
import { policyEngineViolations } from "./policy-engine.mjs"

const adapter = "crates/nessa-auth/src/adapters/cedar/evaluator.rs"
const product = "crates/nessa-server/src/product/read_access.rs"

test("a cedar adapter directory may use the engine and implement the port", () => {
  for (const path of [
    adapter,
    "crates/nessa-server/src/conversation/infrastructure/tool_policy/cedar/mod.rs",
  ])
    assert.deepEqual(
      policyEngineViolations(
        path,
        "use cedar_policy::{Authorizer, PolicySet};\nimpl PolicyEvaluator for CedarPolicyEvaluator {}",
      ),
      [],
    )
})

test("the engine named outside a cedar directory is refused", () => {
  for (const source of [
    "use cedar_policy::Authorizer;",
    "let set = cedar_policy::PolicySet::new();",
    "extern crate cedar_policy;",
    "use cedar_policy as cedar;",
  ])
    assert.equal(policyEngineViolations(product, source).length, 1, source)
})

test("a policy port implemented outside a cedar directory is refused", () => {
  for (const source of [
    "impl PolicyEvaluator for HandWritten {}",
    "impl<T: Send> PolicyEvaluator for HandWritten<T> {}",
    "impl nessa_auth::application::ports::PolicyEvaluator for HandWritten {}",
    "impl ToolPolicyEvaluator for GlobMatcher {}",
  ])
    assert.equal(policyEngineViolations(product, source).length, 1, source)
})

test("a directory merely containing the word is not an adapter", () => {
  for (const path of [
    "crates/nessa-server/src/cedar.rs",
    "crates/nessa-server/src/notcedar/policy.rs",
    "crates/nessa-server/src/cedar_rules/policy.rs",
  ])
    assert.equal(
      policyEngineViolations(path, "impl PolicyEvaluator for HandWritten {}").length,
      1,
      path,
    )
})

test("test code may substitute the port", () => {
  const substitute = "impl PolicyEvaluator for AllowAll {}"
  for (const path of [
    "crates/nessa-server/tests/product/socket/watches.rs",
    "crates/nessa-auth/src/adapters/local/registry/pairing/tests.rs",
    "crates/nessa-auth/src/adapters/local/evaluator_tests.rs",
  ])
    assert.deepEqual(policyEngineViolations(path, substitute), [], path)
  assert.deepEqual(
    policyEngineViolations(
      product,
      `fn product() {}\n#[cfg(test)]\nmod tests {\n    struct AllowAll;\n    impl PolicyEvaluator for AllowAll {\n        fn evaluate(&self) {}\n    }\n}\n`,
    ),
    [],
  )
})

test("code after a test module is still read", () => {
  assert.equal(
    policyEngineViolations(
      product,
      '#[cfg(test)]\nmod tests {\n    fn helper() { let _ = "}"; }\n}\nimpl PolicyEvaluator for HandWritten {}',
    ).length,
    1,
  )
})

test("comments and strings that name the engine are not code", () => {
  assert.deepEqual(
    policyEngineViolations(
      product,
      '// cedar_policy stays in the adapter\nconst NOTE: &str = "impl PolicyEvaluator for X";\n/// See `cedar_policy::Authorizer`.\nfn product() {}',
    ),
    [],
  )
})
