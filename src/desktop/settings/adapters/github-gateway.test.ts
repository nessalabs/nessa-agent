/**
 * The GitHub gateway reads `client.github` into the model's words and gives
 * every refusal its sentence.
 */
import { expect, it } from "vitest"
import {
  GithubRefusalCode,
  NessaGithubError,
  NessaRpcError,
  type GithubApi,
  type GithubConnection,
} from "@nessa/client"
import { githubGateway } from "./github-gateway"

const installUrl = "https://github.com/apps/nessa-agent/installations/new"

function over(answer: () => Promise<GithubConnection>) {
  const github: GithubApi = {
    status: answer,
    connect: answer,
    cancel: answer,
    disconnect: answer,
  }
  return githubGateway({ connected: () => Promise.resolve({ github }) })
}

it("names each state the gateway answers", async () => {
  const connecting = over(() =>
    Promise.resolve({
      state: "connecting",
      code: {
        userCode: "WDJB-MJHT",
        verificationUri: "https://github.com/login/device",
        expiresInSeconds: 899,
      },
      installUrl,
    }),
  )
  expect(await connecting.connect()).toEqual({
    ok: true,
    view: {
      kind: "connecting",
      userCode: "WDJB-MJHT",
      verificationUri: "https://github.com/login/device",
      expiresInSeconds: 899,
      installUrl,
    },
  })
  const connected = over(() =>
    Promise.resolve({
      state: "connected",
      account: { id: 7, login: "octo" },
      installUrl,
    }),
  )
  expect(await connected.read()).toEqual({
    ok: true,
    view: { kind: "connected", login: "octo", installUrl },
  })
  const ended = over(() =>
    Promise.resolve({ state: "signed_out", last: "denied", installUrl }),
  )
  expect(await ended.read()).toEqual({
    ok: true,
    view: { kind: "signedOut", ended: "The sign-in was declined on GitHub.", installUrl },
  })
  const unrecorded = over(() =>
    Promise.resolve({
      state: "signed_out",
      last: "denied",
      unrecorded: true,
      installUrl,
    }),
  )
  expect(await unrecorded.read()).toEqual({
    ok: true,
    view: {
      kind: "signedOut",
      ended:
        "The sign-in was declined on GitHub. Nessa couldn't record this in its audit log.",
      installUrl,
    },
  })
})

it("gives a refusal its sentence, and a lost answer one of its own", async () => {
  const refused = over(() =>
    Promise.reject(
      new NessaGithubError(
        "github.connect",
        new NessaRpcError(GithubRefusalCode.GithubUnreachable, "unreachable"),
      ),
    ),
  )
  expect(await refused.connect()).toEqual({
    ok: false,
    sentence: "Couldn't reach GitHub. Check the connection and try again.",
  })
  const lost = over(() => Promise.reject(new Error("socket closed")))
  expect(await lost.read()).toEqual({
    ok: false,
    sentence: "The gateway didn't answer. Try again.",
  })
  expect(githubGateway({ connected: () => Promise.reject(new Error("no")) }).pollMs).toBe(
    2_000,
  )
})
