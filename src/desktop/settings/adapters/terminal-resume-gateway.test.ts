import { NessaRpcError } from "@nessa/client"
import { expect, it } from "vitest"
import { RuleRefused } from "../model/terminal-resume"
import { terminalResumeGateway } from "./terminal-resume-gateway"

function refusing(cause: unknown) {
  return terminalResumeGateway({
    connected: async () => ({
      terminals: {
        resumeSettings: async () => Promise.reject(cause),
        setResumeSettings: async () => Promise.reject(cause),
        forgetSaved: async () => Promise.reject(cause),
      },
    }),
  })
}

const settings = { enabled: true, rules: [{ program: "x", action: "allow" as const }] }

it("names the part of a rule the gateway refused", async () => {
  const refused = new NessaRpcError("terminal_resume_rule_invalid", "refused", {
    field: "folder",
  })
  await expect(refusing(refused).set(settings)).rejects.toEqual(new RuleRefused("folder"))
  await expect(
    refusing(new NessaRpcError("terminal_resume_rules_limit", "limit")).set(settings),
  ).rejects.toEqual(new RuleRefused("count"))
})

it("passes any other failure on as it came", async () => {
  const lost = new Error("socket closed")
  await expect(refusing(lost).set(settings)).rejects.toBe(lost)
})
