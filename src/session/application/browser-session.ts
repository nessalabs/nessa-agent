/** Local content disappears before waiting for remote sign-out acknowledgement. */
export async function signOutBrowserSession(
  dispose: () => void,
  logout: () => Promise<void>,
) {
  dispose()
  await logout()
}
