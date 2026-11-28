import type { RpcRequester } from "../application/session-port.js"
import {
  AppCapability,
  ProductMethod,
  type AppPackage,
  type AppReview,
  type AppsIssueCredentialResult,
  type InstalledApp,
} from "../generated/product.js"

/**
 * The apps the person always allows: every MCP server whose app was answered
 * "Always allow this app". Such an app's messages go as the person without a
 * review until it is untrusted here. Trust is kept gateway-wide, so only the
 * owner's own surfaces may read or change it.
 */
export type AppsApi = {
  /** The servers whose apps are always allowed, in name order. */
  trusted(): Promise<readonly string[]>
  /**
   * Stop always allowing `server`'s app: its next message is reviewed again.
   * Answers whether it was always allowed.
   */
  untrust(server: string, requestId: string): Promise<boolean>
  /**
   * The starter static app package for a new app, and what the gateway's
   * verifier found in it. Nothing is installed or kept.
   */
  starter(appId: string, packageVersion: string): Promise<CheckedAppPackage>
  /** What the gateway's verifier finds in `pkg`. Nothing is installed or kept. */
  review(pkg: AppPackage): Promise<AppReview>
  /**
   * Install `pkg`, only if its digest is `reviewedDigest`, the one the owner
   * was shown and approved. The gateway verifies the bytes again; a package
   * whose digest differs is refused `app_review_stale` and nothing is kept.
   */
  install(
    pkg: AppPackage,
    reviewedDigest: string,
    requestId: string,
  ): Promise<InstalledApp>
  /** Every installed app, in identity order. Empty means none is installed. */
  installed(): Promise<readonly InstalledApp[]>
  /** One installed app's verified package, to render it. */
  read(appId: string): Promise<CheckedAppPackage>
  /**
   * Remove one installed app and every file it holds; other apps are
   * untouched. Its credentials are revoked first.
   */
  uninstall(appId: string, requestId: string): Promise<void>
  /**
   * Issue the credential `appId` signs in with. It holds exactly what the
   * installed package requests (`AppReview.requestedCapabilities`), and any
   * credential the app held before is revoked. The secret is answered once.
   */
  issueCredential(appId: string, requestId: string): Promise<AppsIssueCredentialResult>
  /** Revoke every credential issued to `appId`; how many were live. */
  revokeCredentials(appId: string, requestId: string): Promise<number>
}

/** A package and what the gateway's verifier found in it. */
export type CheckedAppPackage = {
  readonly package: AppPackage
  readonly review: AppReview
}

const MAX_TRUSTED = 256

export function createAppsApi(session: RpcRequester): AppsApi {
  return {
    trusted: async () => {
      const answer = await session.request(ProductMethod.AppsTrusted, {})
      if (
        !isRecord(answer) ||
        Object.keys(answer).length !== 1 ||
        !Array.isArray(answer.servers) ||
        answer.servers.length > MAX_TRUSTED ||
        !answer.servers.every((server) => typeof server === "string" && server !== "")
      )
        throw new Error("Invalid trusted apps response")
      return answer.servers as string[]
    },
    untrust: async (server, requestId) => {
      const answer = await session.request(ProductMethod.AppsUntrust, {
        requestId,
        server,
      })
      if (
        !isRecord(answer) ||
        Object.keys(answer).length !== 2 ||
        answer.requestId !== requestId ||
        typeof answer.applied !== "boolean"
      )
        throw new Error("Invalid untrust app response")
      return answer.applied
    },
    starter: async (appId, packageVersion) =>
      checkedPackage(
        await session.request(ProductMethod.AppsStarter, { appId, packageVersion }),
      ),
    review: async (pkg) =>
      appReview(await session.request(ProductMethod.AppsReview, { package: pkg })),
    install: async (pkg, reviewedDigest, requestId) => {
      const answer = await session.request(ProductMethod.AppsInstall, {
        requestId,
        package: pkg,
        reviewedDigest,
      })
      if (
        !isRecord(answer) ||
        !exactKeys(answer, ["requestId", "app"]) ||
        answer.requestId !== requestId
      )
        throw new Error("Invalid app install response")
      return installedApp(answer.app)
    },
    installed: async () => {
      const answer = await session.request(ProductMethod.AppsInstalled, {})
      if (
        !isRecord(answer) ||
        !exactKeys(answer, ["apps"]) ||
        !Array.isArray(answer.apps) ||
        answer.apps.length > MAX_INSTALLED
      )
        throw new Error("Invalid installed apps response")
      return answer.apps.map(installedApp)
    },
    read: async (appId) => {
      const checked = checkedPackage(
        await session.request(ProductMethod.AppsRead, { appId }),
      )
      if (checked.review.appId !== appId) throw new Error("Invalid app read response")
      return checked
    },
    uninstall: async (appId, requestId) => {
      const answer = await session.request(ProductMethod.AppsUninstall, {
        requestId,
        appId,
      })
      if (
        !isRecord(answer) ||
        !exactKeys(answer, ["requestId", "appId"]) ||
        answer.requestId !== requestId ||
        answer.appId !== appId
      )
        throw new Error("Invalid app uninstall response")
    },
    issueCredential: async (appId, requestId) => {
      const answer = await session.request(ProductMethod.AppsIssueCredential, {
        requestId,
        appId,
      })
      if (
        !isRecord(answer) ||
        !exactKeys(answer, [
          "requestId",
          "appId",
          "credentialId",
          "credential",
          "expiresAt",
          "capabilities",
        ]) ||
        answer.requestId !== requestId ||
        answer.appId !== appId ||
        !text(answer.credentialId) ||
        !text(answer.credential) ||
        !Number.isSafeInteger(answer.expiresAt) ||
        (answer.expiresAt as number) < 0
      )
        throw new Error("Invalid app credential response")
      const capabilities = capabilityList(answer.capabilities)
      if (capabilities.length === 0) throw new Error("Invalid app credential response")
      return {
        requestId,
        appId,
        credentialId: answer.credentialId,
        credential: answer.credential,
        expiresAt: answer.expiresAt as number,
        capabilities,
      }
    },
    revokeCredentials: async (appId, requestId) => {
      const answer = await session.request(ProductMethod.AppsRevokeCredentials, {
        requestId,
        appId,
      })
      if (
        !isRecord(answer) ||
        !exactKeys(answer, ["requestId", "appId", "revoked"]) ||
        answer.requestId !== requestId ||
        answer.appId !== appId ||
        !Number.isSafeInteger(answer.revoked) ||
        (answer.revoked as number) < 0
      )
        throw new Error("Invalid app credential revoke response")
      return answer.revoked as number
    },
  }
}

const CAPABILITIES: readonly string[] = Object.values(AppCapability)

/** A capability list as the gateway answers it: known names, none twice. */
function capabilityList(value: unknown): AppCapability[] {
  if (
    !Array.isArray(value) ||
    value.length > CAPABILITIES.length ||
    !value.every((item) => typeof item === "string" && CAPABILITIES.includes(item)) ||
    new Set(value).size !== value.length
  )
    throw new Error("Invalid app capabilities")
  return value as AppCapability[]
}

const MAX_INSTALLED = 256
const MAX_ASSETS = 8
const DIGEST = /^[0-9a-f]{64}$/

function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const own = Object.keys(value)
  return own.length === keys.length && keys.every((key) => Object.hasOwn(value, key))
}

function text(value: unknown): value is string {
  return typeof value === "string" && value !== ""
}

function installedApp(value: unknown): InstalledApp {
  if (
    !isRecord(value) ||
    !exactKeys(value, ["appId", "packageVersion"]) ||
    !text(value.appId) ||
    !text(value.packageVersion)
  )
    throw new Error("Invalid installed app")
  return { appId: value.appId, packageVersion: value.packageVersion }
}

function appReview(value: unknown): AppReview {
  if (
    !isRecord(value) ||
    !exactKeys(value, [
      "appId",
      "packageVersion",
      "entrypoint",
      "assetCount",
      "digest",
      "requestedCapabilities",
    ]) ||
    !text(value.appId) ||
    !text(value.packageVersion) ||
    !text(value.entrypoint) ||
    !Number.isInteger(value.assetCount) ||
    (value.assetCount as number) < 1 ||
    (value.assetCount as number) > MAX_ASSETS ||
    typeof value.digest !== "string" ||
    !DIGEST.test(value.digest)
  )
    throw new Error("Invalid app review")
  return {
    appId: value.appId,
    packageVersion: value.packageVersion,
    entrypoint: value.entrypoint,
    assetCount: value.assetCount as number,
    digest: value.digest,
    requestedCapabilities: capabilityList(value.requestedCapabilities),
  }
}

function appPackage(value: unknown): AppPackage {
  if (
    !isRecord(value) ||
    !exactKeys(value, ["manifest", "assets"]) ||
    typeof value.manifest !== "string" ||
    !Array.isArray(value.assets) ||
    value.assets.length > MAX_ASSETS
  )
    throw new Error("Invalid app package")
  return {
    manifest: value.manifest,
    assets: value.assets.map((asset: unknown) => {
      if (
        !isRecord(asset) ||
        !exactKeys(asset, ["path", "bytes"]) ||
        !text(asset.path) ||
        typeof asset.bytes !== "string"
      )
        throw new Error("Invalid app package asset")
      return { path: asset.path, bytes: asset.bytes }
    }),
  }
}

function checkedPackage(value: unknown): CheckedAppPackage {
  if (!isRecord(value) || !exactKeys(value, ["package", "review"]))
    throw new Error("Invalid app package response")
  return { package: appPackage(value.package), review: appReview(value.review) }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}
