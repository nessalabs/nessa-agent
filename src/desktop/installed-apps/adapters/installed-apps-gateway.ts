/**
 * Installed apps, for Settings › Apps: the window's client (`client.apps`),
 * which checks each answer against the protocol, read as it is. The gateway
 * is the one authority on what is installed; this only asks it.
 */
import {
  NessaRpcError,
  type AppPackage,
  type AppReview,
  type AppsApi,
  type CheckedAppPackage,
  type InstalledApp,
} from "@nessa/client"

/** What Settings asks of the window's gateway client. */
export interface InstalledAppsClient {
  readonly apps: Pick<
    AppsApi,
    "starter" | "review" | "install" | "installed" | "read" | "uninstall"
  >
}

export interface InstalledAppsGateway {
  starter(appId: string, packageVersion: string): Promise<CheckedAppPackage>
  review(pkg: AppPackage): Promise<AppReview>
  install(
    pkg: AppPackage,
    reviewedDigest: string,
    requestId: string,
  ): Promise<InstalledApp>
  installed(): Promise<readonly InstalledApp[]>
  read(appId: string): Promise<CheckedAppPackage>
  uninstall(appId: string, requestId: string): Promise<void>
}

export function installedAppsGateway(
  connected: () => Promise<InstalledAppsClient>,
): InstalledAppsGateway {
  return {
    starter: async (appId, version) => (await connected()).apps.starter(appId, version),
    review: async (pkg) => (await connected()).apps.review(pkg),
    install: async (pkg, digest, requestId) =>
      (await connected()).apps.install(pkg, digest, requestId),
    installed: async () => (await connected()).apps.installed(),
    read: async (appId) => (await connected()).apps.read(appId),
    uninstall: async (appId, requestId) =>
      (await connected()).apps.uninstall(appId, requestId),
  }
}

/** What a refusal means to the person, in a sentence; generic otherwise. */
export function refusalWords(cause: unknown): string {
  const code = cause instanceof NessaRpcError ? cause.code : undefined
  switch (code) {
    case "app_package_invalid":
      return "That isn’t a valid app id and version. Use lowercase letters, digits, hyphens and underscores, such as my-notes, and a version such as 1.0.0."
    case "app_package_too_large":
      return "This app is too large to install here."
    case "app_review_stale":
      return "The app changed after you reviewed it. Review it again."
    case "app_already_installed":
      return "An app with this id is already installed. Uninstall it first."
    case "app_not_installed":
      return "This app isn’t installed any more."
    case "apps_not_configured":
    case "apps_storage_unavailable":
    case "apps_audit_unavailable":
      return "Apps aren’t available on this gateway right now."
    default:
      return "That didn’t work. Try again."
  }
}
