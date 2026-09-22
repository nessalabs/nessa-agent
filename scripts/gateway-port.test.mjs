import assert from "node:assert/strict"
import test from "node:test"

import { gatewayPort, selectedPort, selectedStage } from "./gateway-port.mjs"

/**
 * These answer for `nessa-server`, which reads `NESSA_STAGE` and treats it as
 * `dev` when it says nothing, and `NESSA_PORT` ahead of the stage's own port.
 * Anything that starts that server and then waits for it — `just start` frees a
 * socket, launches, and probes for health — has to name the same socket the
 * server will. Assuming `dev` meant a `ci` run freed 7421, started a gateway on
 * 7420, and waited for health on 7421 until it gave up.
 */
test("no stage named is the dev stage, the way the server reads it", () => {
  assert.equal(selectedStage({}), "dev")
  assert.equal(selectedStage({ NESSA_STAGE: "" }), "dev")
  assert.equal(selectedPort({}), gatewayPort("dev"))
})

test("the stage the environment selects is the one answered for", () => {
  assert.equal(selectedStage({ NESSA_STAGE: "ci" }), "ci")
  assert.equal(selectedPort({ NESSA_STAGE: "ci" }), gatewayPort("ci"))
  assert.equal(selectedPort({ NESSA_STAGE: "prod" }), gatewayPort("prod"))
  // Surrounding whitespace is a shell artefact, not a different stage.
  assert.equal(selectedStage({ NESSA_STAGE: " ci " }), "ci")
})

/**
 * The server matches stage names exactly (`Stage::parse`), and the host's
 * launchd registration looks the name up in this same table. A rule here that
 * accepted more than the server does would answer 7421 for `Dev`, free and
 * probe that socket, and leave the server refusing to start — three different
 * failures from one value, none of them naming the cause.
 */
test("a stage the server would refuse is refused here, in the same words", () => {
  for (const named of ["Dev", "DEV", "Prod"]) {
    assert.throws(() => selectedStage({ NESSA_STAGE: named }), /is not a stage/)
  }
  assert.throws(() => selectedStage({ NESSA_STAGE: "staging" }), /dev, ci, alpha, prod/)
})

/**
 * `in` walks the prototype chain, so the guard above was true for every name
 * `Object.prototype` carries and `NESSA_STAGE=toString` came out of here as a
 * stage — one `just start` frees a socket for, registers under, and probes.
 * The same value in `src/env/gateway-ports.ts` reached `gatewayOrigin` and
 * built a URL out of a function; these two parsers read one variable against
 * one table and have to refuse the same things.
 */
test("a name every object answers to is not a stage here either", () => {
  for (const named of [
    "toString",
    "constructor",
    "valueOf",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "__proto__",
  ]) {
    assert.throws(() => selectedStage({ NESSA_STAGE: named }), /is not a stage/)
    assert.throws(() => selectedPort({ NESSA_STAGE: named }), /is not a stage/)
    // `node scripts/gateway-port.mjs <stage>` reaches the table with no guard
    // in front of it at all.
    assert.throws(() => gatewayPort(named), /No gateway port/)
  }
})

test("an explicit port wins, because it wins for the server too", () => {
  assert.equal(selectedPort({ NESSA_PORT: "7999" }), 7999)
  assert.equal(selectedPort({ NESSA_STAGE: "ci", NESSA_PORT: "7999" }), 7999)
  assert.equal(selectedPort({ NESSA_PORT: "  " }), gatewayPort("dev"))
})

test("a stage or port that is not one is refused rather than guessed", () => {
  assert.throws(() => selectedPort({ NESSA_PORT: "no" }), /not a port/)
  assert.throws(() => selectedPort({ NESSA_PORT: "70000" }), /not a port/)
})
