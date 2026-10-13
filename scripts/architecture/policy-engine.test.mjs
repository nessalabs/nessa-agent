import assert from "node:assert/strict"
import test from "node:test"
import { isTestCode, policyEngineViolations, testModuleFiles } from "./policy-engine.mjs"

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

test("the engine named outside a cedar directory is refused, in test code too", () => {
  for (const source of [
    "use cedar_policy::Authorizer;",
    "let set = cedar_policy::PolicySet::new();",
    "extern crate cedar_policy;",
    "use cedar_policy as cedar;",
    "#[cfg(test)]\nmod tests {\n    use cedar_policy::Authorizer;\n}",
  ])
    assert.equal(policyEngineViolations(product, source).length, 1, source)
  assert.equal(
    policyEngineViolations(product, "use cedar_policy::Authorizer;", true).length,
    1,
  )
})

test("a policy port implemented outside a cedar directory is refused", () => {
  for (const source of [
    "impl PolicyEvaluator for HandWritten {}",
    "impl<T: Send> PolicyEvaluator for HandWritten<T> {}",
    "impl nessa_auth::application::ports::PolicyEvaluator for HandWritten {}",
    "impl ToolPolicyEvaluator for GlobMatcher {}",
    "impl PolicyEvaluator<Request> for HandWritten {}",
    "impl<T> ports::ToolPolicyEvaluator<T, Vec<u8>> for HandWritten {}",
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

test("an inline test module may substitute the port", () => {
  for (const attributes of [
    "#[cfg(test)]",
    "#[cfg(test)]\n#[allow(dead_code)]",
    "#[cfg(all(test, unix))]",
  ])
    assert.deepEqual(
      policyEngineViolations(
        product,
        `fn product() {}\n${attributes}\nmod tests {\n    struct AllowAll;\n    impl PolicyEvaluator for AllowAll {\n        fn evaluate(&self) {}\n    }\n}\n`,
      ),
      [],
      attributes,
    )
})

test("a file the caller marks as test code may substitute the port", () => {
  assert.deepEqual(
    policyEngineViolations(
      "crates/nessa-server/src/conversation/test_support.rs",
      "impl PolicyEvaluator for AllowAll {}",
      true,
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

test("out-of-line test modules resolve the way the compiler loads them", () => {
  assert.deepEqual(
    testModuleFiles(
      "crates/nessa-server/src/conversation/service.rs",
      "#[cfg(test)]\nmod test_support;",
    ),
    {
      files: [
        "crates/nessa-server/src/conversation/service/test_support.rs",
        "crates/nessa-server/src/conversation/service/test_support/mod.rs",
      ],
      below: ["crates/nessa-server/src/conversation/service/test_support/"],
    },
  )
  assert.deepEqual(
    testModuleFiles(
      "crates/nessa-server/src/browser_session/mod.rs",
      "#[cfg(test)]\nmod tests;",
    ),
    {
      files: [
        "crates/nessa-server/src/browser_session/tests.rs",
        "crates/nessa-server/src/browser_session/tests/mod.rs",
      ],
      below: ["crates/nessa-server/src/browser_session/tests/"],
    },
  )
  assert.deepEqual(
    testModuleFiles(
      "crates/nessa-auth/src/adapters/cedar/evaluator.rs",
      '#[cfg(test)]\n#[path = "evaluator_tests.rs"]\nmod tests;',
    ),
    {
      files: ["crates/nessa-auth/src/adapters/cedar/evaluator_tests.rs"],
      below: ["crates/nessa-auth/src/adapters/cedar/evaluator_tests/"],
    },
  )
})

test("a module that is not cfg(test), or a declaration in a comment, is product code", () => {
  for (const source of [
    "mod helpers;",
    "// #[cfg(test)]\n// mod tests;",
    "#[cfg(unix)]\nmod unix;",
  ])
    assert.deepEqual(testModuleFiles(product, source), { files: [], below: [] }, source)
})

test("everything below a test module is test code", () => {
  const tests = testModuleFiles(
    "crates/nessa-server/src/conversation/service.rs",
    "#[cfg(test)]\nmod test_support;",
  )
  const known = { files: new Set(tests.files), below: tests.below }
  assert.equal(
    isTestCode("crates/nessa-server/src/conversation/service/test_support.rs", known),
    true,
  )
  assert.equal(
    isTestCode(
      "crates/nessa-server/src/conversation/service/test_support/fakes.rs",
      known,
    ),
    true,
  )
  assert.equal(
    isTestCode("crates/nessa-server/src/conversation/service/other.rs", known),
    false,
  )
})
