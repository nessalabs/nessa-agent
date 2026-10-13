import assert from "node:assert/strict"
import test from "node:test"
import {
  moduleDeclarations,
  policyEngineDependencyViolations,
  policyEngineViolations,
  testCodeFiles,
} from "./policy-engine.mjs"

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

const paths = (modules) => modules.map((candidates) => candidates.map((c) => c.path))

test("out-of-line modules resolve the way rustc loads them", () => {
  assert.deepEqual(
    paths(
      moduleDeclarations(
        "crates/nessa-server/src/conversation/service.rs",
        "#[cfg(test)]\nmod test_support;",
      ),
    ),
    [
      [
        "crates/nessa-server/src/conversation/service/test_support.rs",
        "crates/nessa-server/src/conversation/service/test_support/mod.rs",
      ],
    ],
  )
  assert.deepEqual(
    paths(
      moduleDeclarations("crates/nessa-server/src/browser_session/mod.rs", "mod tests;"),
    ),
    [
      [
        "crates/nessa-server/src/browser_session/tests.rs",
        "crates/nessa-server/src/browser_session/tests/mod.rs",
      ],
    ],
  )
  assert.deepEqual(
    paths(
      moduleDeclarations(
        "crates/nessa-auth/src/adapters/cedar/evaluator.rs",
        '#[cfg(test)]\n#[path = "../../../tests/cedar.rs"]\nmod tests;',
      ),
    ),
    [["crates/nessa-auth/tests/cedar.rs"]],
  )
  // A file loaded through #[path] owns its own directory, like a mod.rs.
  assert.deepEqual(
    paths(
      moduleDeclarations(
        "crates/nessa-auth/src/adapters/cedar/evaluator_tests.rs",
        "mod helpers;",
        {
          ownsDirectory: true,
        },
      ),
    ),
    [
      [
        "crates/nessa-auth/src/adapters/cedar/helpers.rs",
        "crates/nessa-auth/src/adapters/cedar/helpers/mod.rs",
      ],
    ],
  )
})

test("only test cfgs count when asked, and a declaration in a comment is not one", () => {
  for (const source of [
    "mod helpers;",
    "// #[cfg(test)]\n// mod tests;",
    "#[cfg(unix)]\nmod unix;",
    "#[cfg(not(test))]\nmod real;",
  ])
    assert.deepEqual(moduleDeclarations(product, source, { testOnly: true }), [], source)
  for (const source of [
    "#[cfg(test)]\nmod tests;",
    "#[cfg(test)]\n#[allow(dead_code)]\nmod tests;",
    "#[cfg(all(test, unix))]\npub(crate) mod tests;",
  ])
    assert.equal(
      moduleDeclarations(product, source, { testOnly: true }).length,
      1,
      source,
    )
})

test("test code is what cfg(test) loads, and everything that declares in turn", () => {
  const sources = new Map([
    ["crates/x/src/lib.rs", "mod service;\n#[cfg(test)]\nmod test_support;"],
    ["crates/x/src/service.rs", '#[cfg(test)]\n#[path = "service_tests.rs"]\nmod tests;'],
    ["crates/x/src/service_tests.rs", "mod fakes;"],
    ["crates/x/src/fakes.rs", ""],
    ["crates/x/src/test_support/mod.rs", "mod doubles;"],
    ["crates/x/src/test_support/doubles.rs", ""],
  ])
  assert.deepEqual([...testCodeFiles(sources)].sort(), [
    "crates/x/src/fakes.rs",
    "crates/x/src/service_tests.rs",
    "crates/x/src/test_support/doubles.rs",
    "crates/x/src/test_support/mod.rs",
  ])
})

test("a file product code also loads is product code", () => {
  const sources = new Map([
    ["crates/x/src/lib.rs", "mod shared;\n#[cfg(test)]\nmod tests;"],
    ["crates/x/src/tests.rs", '#[path = "shared.rs"]\nmod shared;'],
    ["crates/x/src/shared.rs", "mod inner;"],
    ["crates/x/src/shared/inner.rs", ""],
  ])
  assert.deepEqual([...testCodeFiles(sources)].sort(), ["crates/x/src/tests.rs"])
})

test("a declaration nested in an inline module is not read as a top-level one", () => {
  for (const source of [
    "mod outer {\n    #[cfg(test)]\n    mod tests;\n}",
    "#[cfg(test)]\nmod tests {\n    mod helpers;\n}",
  ])
    assert.deepEqual(moduleDeclarations(product, source), [], source)
  assert.equal(
    moduleDeclarations(
      product,
      "mod outer {\n    fn f() {}\n}\n#[cfg(test)]\nmod tests;",
      {
        testOnly: true,
      },
    ).length,
    1,
  )
})

test("a policy port implemented under an alias is refused", () => {
  for (const source of [
    "use nessa_auth::application::ports::PolicyEvaluator as Rules;\nimpl Rules for HandWritten {}",
    "use ports::{ToolPolicyEvaluator as Tools, Decision};\nimpl<T> Tools<T> for HandWritten {}",
  ])
    assert.equal(policyEngineViolations(product, source).length, 1, source)
  assert.deepEqual(
    policyEngineViolations(
      product,
      "use ports::PolicyEvaluator as _;\nimpl Display for X {}",
    ),
    [],
  )
})

test("a policy port keeps its name, so a re-export cannot hide it", () => {
  for (const source of [
    "pub use crate::ports::PolicyEvaluator as Rules;",
    "pub use ports::{ToolPolicyEvaluator as Tools};",
  ])
    assert.equal(policyEngineViolations(product, source).length, 1, source)
})

test("test anywhere among all(...)'s operands is a test cfg; not(test) and any(...) are not", () => {
  const body = "mod tests {\n    impl PolicyEvaluator for AllowAll {}\n}"
  for (const cfg of [
    "#[cfg(all(unix, test))]",
    '#[cfg(all(unix, all(feature = "x", test)))]',
  ])
    assert.deepEqual(policyEngineViolations(product, `${cfg}\n${body}`), [], cfg)
  for (const cfg of [
    "#[cfg(all(unix, not(test)))]",
    "#[cfg(any(test, unix))]",
    "#[cfg(not(test))]",
  ])
    assert.equal(policyEngineViolations(product, `${cfg}\n${body}`).length, 1, cfg)
  assert.equal(
    moduleDeclarations(product, "#[cfg(all(unix, test))]\nmod tests;", { testOnly: true })
      .length,
    1,
  )
})

test("a Cargo target root is product code even when a test module also loads it", () => {
  const sources = new Map([
    ["crates/x/src/lib.rs", '#[cfg(test)]\n#[path = "main.rs"]\nmod binary;'],
    ["crates/x/src/main.rs", ""],
  ])
  assert.deepEqual([...testCodeFiles(sources)], ["crates/x/src/main.rs"])
  assert.deepEqual(
    [...testCodeFiles(sources, ["crates/x/src/lib.rs", "crates/x/src/main.rs"])],
    [],
  )
})

test("the engine dependency is never renamed", () => {
  assert.deepEqual(
    policyEngineDependencyViolations([
      { name: "cedar-policy", rename: null },
      { name: "serde", rename: "serde_renamed" },
    ]),
    [],
  )
  assert.equal(
    policyEngineDependencyViolations([{ name: "cedar-policy", rename: "rules" }]).length,
    1,
  )
})

test("a path with spaces around :: still names the port", () => {
  for (const source of [
    "impl crate :: ports :: PolicyEvaluator for Stub {}",
    "impl ::ports::PolicyEvaluator for Stub {}",
    "impl PolicyEvaluator<T>for Stub {}",
  ])
    assert.equal(policyEngineViolations(product, source).length, 1, source)
})

test("any item behind a test cfg may substitute the port, an ungated one may not", () => {
  for (const source of [
    "#[cfg(test)]\nimpl PolicyEvaluator for Stub {\n    fn evaluate(&self) {}\n}",
    "#[cfg(test)]\nfn stub() {\n    struct S;\n    impl PolicyEvaluator for S {}\n}",
  ])
    assert.deepEqual(policyEngineViolations(product, source), [], source)
  assert.equal(
    policyEngineViolations(
      product,
      "#[cfg(test)]\nuse fakes::Stub;\n#[derive(Debug)]\nstruct Real;\nimpl PolicyEvaluator for Real {}",
    ).length,
    1,
  )
})
