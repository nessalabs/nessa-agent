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
  // Written as somebody would type it.
  assert.equal(selectedStage({ NESSA_STAGE: " CI " }), "ci")
})

test("an explicit port wins, because it wins for the server too", () => {
  assert.equal(selectedPort({ NESSA_PORT: "7999" }), 7999)
  assert.equal(selectedPort({ NESSA_STAGE: "ci", NESSA_PORT: "7999" }), 7999)
  assert.equal(selectedPort({ NESSA_PORT: "  " }), gatewayPort("dev"))
})

test("a stage or port that is not one is refused rather than guessed", () => {
  assert.throws(() => selectedStage({ NESSA_STAGE: "staging" }), /staging/)
  assert.throws(() => selectedPort({ NESSA_PORT: "no" }), /not a port/)
  assert.throws(() => selectedPort({ NESSA_PORT: "70000" }), /not a port/)
})
