// @vitest-environment jsdom
/**
 * The window's GitHub picture: asked for from GitHub's avatar host by the
 * account's id, never for an unknown account, and the login's initial when
 * the picture cannot load.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { githubAvatar, githubAvatarUrl } from "./github-avatar"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

const Avatar = githubAvatar

it("asks GitHub for the account's picture by its id", async () => {
  expect(githubAvatarUrl(27861809)).toBe(
    "https://avatars.githubusercontent.com/u/27861809?s=56&v=4",
  )
  await act(async () => root.render(<Avatar id={7} login="octo" />))
  expect(host.querySelector("img")?.getAttribute("src")).toBe(githubAvatarUrl(7))
})

it("asks for nothing for an unknown account, and draws the initial", async () => {
  await act(async () => root.render(<Avatar id={0} login="octo" />))
  expect(host.querySelector("img")).toBeNull()
  expect(host.textContent).toBe("O")
})

it("draws the initial when the picture fails to load", async () => {
  await act(async () => root.render(<Avatar id={7} login="octo" />))
  await act(async () => {
    host.querySelector("img")?.dispatchEvent(new Event("error"))
  })
  expect(host.querySelector("img")).toBeNull()
  expect(host.textContent).toBe("O")
})
