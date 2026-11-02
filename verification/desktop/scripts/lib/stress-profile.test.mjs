import assert from "node:assert/strict"
import { test } from "node:test"
import {
  stressProfile,
  selectStressFixture,
  requireStressCapabilities,
  elevatedAccessAfter,
} from "./stress-profile.mjs"
import { knobsFrom, plan } from "./stress-plan.mjs"
import { scriptedLaunch } from "./gateway-stack.mjs"
import { CannotRun } from "./cli.mjs"

const row = (modelId, modes) => ({ modelId, approvalModes: modes.map((id) => ({ id })) })
test("explicit scenario and exact model never silently replace the shared defaults", () => {
  assert.deepEqual(stressProfile({ agent: "codex" }), {
    agent: "codex",
    model: "gpt-5.6-terra",
    scenario: "full",
  })
  const profile = stressProfile({
    agent: "codex",
    model: "gpt-6-astra",
    accessScenario: "full",
  })
  assert.equal(
    scriptedLaunch(profile.agent, { scenario: "fixture.json", model: profile.model })
      .command.model,
    "gpt-6-astra",
  )
  assert.equal(
    scriptedLaunch("codex", { scenario: "fixture.json" }).command.model,
    "gpt-5.6-terra",
  )
  assert.throws(() => stressProfile({ agent: "claude", model: "gpt-6-astra" }), /catalog/)
  assert.throws(() => stressProfile({ agent: "codex", model: "unknown" }), /catalog/)
  assert.throws(
    () => stressProfile({ agent: "codex", accessScenario: "automatic" }),
    /scenario/,
  )
})
test("only pre-workload incompatible fixtures are classified unavailable", () => {
  const full = stressProfile({ agent: "codex", model: "gpt-6-astra" })
  const agents = [{ agent: "codex", models: [row(full.model, ["ask"])] }]
  assert.throws(() => selectStressFixture(full, agents), CannotRun)
  assert.throws(() => selectStressFixture(full, []), CannotRun)
  assert.throws(
    () => requireStressCapabilities(full, agents[0].models[0]),
    (error) => !(error instanceof CannotRun),
  )
  agents[0].models[0] = row(full.model, ["ask", "auto", "full"])
  assert.equal(selectStressFixture(full, agents).model, full.model)
})
test("fixture admission requires the exact published scenario capabilities and identity", () => {
  const full = stressProfile({ agent: "codex", model: "gpt-6-astra" })
  const ask = stressProfile({ agent: "codex", accessScenario: "ask-only" })
  assert.deepEqual(
    requireStressCapabilities(full, row(full.model, ["ask", "auto", "full"]))
      .requiredModes,
    ["ask", "auto", "full"],
  )
  assert.deepEqual(
    requireStressCapabilities(ask, row(ask.model, ["ask"])).requiredModes,
    ["ask"],
  )
  assert.throws(
    () => requireStressCapabilities(full, row(full.model, ["ask"])),
    /incompatible/,
  )
  assert.throws(
    () => requireStressCapabilities(ask, row(ask.model, ["ask", "auto", "full"])),
    /incompatible/,
  )
  assert.throws(
    () => requireStressCapabilities(full, row("gpt-5.6-terra", ["ask", "auto", "full"])),
    /differs/,
  )
  assert.throws(() => requireStressCapabilities(full), /differs/)
  for (const modes of [
    ["ask", "full"],
    ["auto", "full"],
    ["ask", "ask", "full"],
  ])
    assert.throws(
      () => requireStressCapabilities(full, row(full.model, modes)),
      /incompatible/,
    )
})
test("Ask-only stress keeps all counted work and witnesses each refusal with an Ask permission", () => {
  for (let seed = 0; seed < 50; seed++) {
    for (const switches of [3, 9]) {
      const knobs = knobsFrom(
        {
          seed: String(seed),
          switches: String(switches),
          parentstops: "3",
          reviews: "30",
          rounds: "66",
        },
        {},
      )
      const steps = plan(knobs, "ask-only")
      assert.equal(
        steps.filter((step) => step.kind === "refused-access").length,
        switches,
      )
      assert.equal(steps.filter((step) => step.kind === "parent-stop").length, 3)
      assert.equal(
        steps.filter((step) => step.kind === "model").length,
        knobs.modelSwitches,
      )
      assert.equal(
        steps.some((step) => step.kind === "switch"),
        false,
      )
      for (const [i, step] of steps.entries()) {
        if (step.kind === "refused-access") {
          assert.equal(step.from, "ask")
          assert.equal(step.to, "full")
          assert.equal(steps[i + 1].kind, "permission")
          assert.equal(steps[i + 1].mode, "ask")
        }
        if (step.mode) assert.equal(step.mode, "ask")
      }
    }
  }
  const knobs = knobsFrom({}, {})
  assert.deepEqual(plan(knobs, "full"), plan(knobs))
  assert.throws(() => plan(knobs, "automatic"), /scenario/)
})
const record = (params, method = "session/set_config_option") =>
  JSON.stringify({
    direction: "to-agent",
    frame: { jsonrpc: "2.0", id: 1, method, params: { sessionId: "native", ...params } },
  }) + "\n"
test("refusal side-effect evidence rejects elevated provider configuration and damaged recording", () => {
  const before = record({ configId: "mode", value: "read-only" })
  assert.equal(elevatedAccessAfter(before, before), false)
  assert.equal(
    elevatedAccessAfter(
      before,
      before + record({ configId: "model", value: "gpt-5.6-terra" }),
    ),
    false,
  )
  for (const mode of ["agent", "agent-full-access"])
    assert.equal(
      elevatedAccessAfter(before, before + record({ configId: "mode", value: mode })),
      true,
    )
  assert.equal(
    elevatedAccessAfter(
      before,
      before + record({ modeId: "agent-full-access" }, "session/set_mode"),
    ),
    true,
  )
  assert.throws(() => elevatedAccessAfter(before, ""), /recording/)
  assert.throws(() => elevatedAccessAfter(before, before + "{"), /recording/)
  assert.throws(() => elevatedAccessAfter(before, before + "{}\n"), /recording/)
  assert.throws(() => elevatedAccessAfter("", ""), /recording/)
  for (const frame of [
    { jsonrpc: "2.0" },
    { jsonrpc: "2.0", id: 1 },
    { jsonrpc: "2.0", id: 1, result: {}, error: {} },
    {
      jsonrpc: "2.0",
      method: "session/set_config_option",
      params: { configId: "mode", value: "agent-full-access" },
    },
    {
      jsonrpc: "2.0",
      method: "session/set_config_option",
      params: { sessionId: "native", configId: "mode" },
    },
    { jsonrpc: "2.0", method: "session/set_mode", params: { sessionId: "native" } },
  ])
    assert.throws(
      () =>
        elevatedAccessAfter(
          before,
          before + JSON.stringify({ direction: "to-agent", frame }) + "\n",
        ),
      /ACP/,
    )
  for (const frame of [
    { jsonrpc: "2.0", id: 1, result: {} },
    { jsonrpc: "2.0", method: "session/update", params: {} },
  ])
    assert.equal(
      elevatedAccessAfter(
        before,
        before + JSON.stringify({ direction: "from-agent", frame }) + "\n",
      ),
      false,
    )
})
