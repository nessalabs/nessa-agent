import { ContactAvatar } from "@nessa-ui/react/message-actions/contact-avatar"
import type { GithubAvatar } from "../ui/github-row"

/**
 * Where GitHub serves an account's picture, by its numeric id, which never
 * changes (a login can): at twice the row's 28px, for a sharp edge.
 */
export function githubAvatarUrl(id: number): string {
  return `https://avatars.githubusercontent.com/u/${id}?s=56&v=4`
}

/**
 * The window's `GithubAvatar`: GitHub's picture, or the login's initial for an
 * unknown account (id 0, asked for nothing) or a picture that fails to load.
 */
export const githubAvatar: GithubAvatar = ({ id, login }) => (
  <ContactAvatar
    name={login}
    src={id > 0 ? githubAvatarUrl(id) : undefined}
    className="settings-github-avatar"
  />
)
