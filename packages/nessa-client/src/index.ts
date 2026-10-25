/**
 * @nessa/client — typed WebSocket SDK for the Nessa Server Client API.
 *
 * ```text
 *                    connectClient (composition)
 *                           │
 *           ┌───────────────┼───────────────┐
 *           │               │               │
 *    presentation      application     transport
 *    (NessaClient)     (handshake)     (WireSession)
 *           │               │               │
 *           └───────────────┴───────────────┘
 *                           │
 *                      protocol (typed frames)
 * ```
 */
export {
  NessaClient,
  createSubscriptionGate,
  createConversationReadTools,
  type ConversationReadToolDescriptor,
  type ConversationReadTools,
  type ConversationReadToolsOptions,
  type SubscriptionGate,
  type AuthApi,
  type CredentialApi,
  type CredentialGrant,
  type CredentialMetadata,
  type IssueCredentialParams,
  type IssueCredentialResult,
  type ServerApi,
  type WorkspaceApi,
  type ChannelApi,
  channelRefusal,
  type AgentsApi,
  type RecordReadApi,
  type AgentsSubscriptionHandlers,
  type ListSubscriptionHandlers,
  type Subscription,
  type SubscriptionApi,
  type SubscriptionEnd,
  type ViewFrame,
  type ViewSubscriptionHandlers,
  type EventSubscriptionHandlers,
} from "./presentation/index.js"
export type {
  NessaClientConnectOptions,
  ProductConnectOptions,
  GatewayEndpointContext,
  GatewayEndpointSource,
} from "./application/index.js"
export type { NessaClientEvents } from "./application/index.js"
export {
  isLoopbackWebSocketUrl,
  isStage,
  NessaRpcError,
  NessaProtocolCompatibilityError,
  NessaConnectionClosedError,
  RetryableConnectError,
  NessaEndpointDiscoveryError,
  resolveConnectOptions,
  stageAllowsDefaultUrl,
  StageConfigError,
  STAGES,
  Stage,
} from "./application/index.js"
export type {
  ClientInfo,
  EventFrame,
  Frame,
  GatewayError,
  HealthResult,
  ReqFrame,
  ResFrame,
  ShortcutArgs,
  ShortcutBinding,
  ShortcutsDocument,
  SurfaceInfo,
  ProductSessionReady,
  SessionAuthenticateParams,
  SessionChallenge,
} from "./protocol/index.js"
export type { EventName, MethodName } from "./protocol/index.js"
export { Event, Method } from "./protocol/index.js"
export { ProductEvent, ProductMethod } from "./protocol/index.js"

export type { ProductConnectRetryOptions } from "./application/connect-retry.js"

export { NessaClientConfig, type NessaClientConfigOptions } from "./application/index.js"

export {
  PrincipalKind,
  MembershipRole,
  MembershipState,
  SessionCloseReason,
} from "./generated/product.js"
export {
  NessaSessionUnavailableError,
  type ConnectionState,
} from "./application/managed-session.js"

export {
  ClientRole,
  ClientPlatform,
  SurfaceKind,
  Scope,
  ShortcutAction,
  ShortcutScope,
  ShortcutSurface,
} from "./generated/protocol.js"
export const ConnectionProfile = { Product: "product" } as const

export type { CommonConnectOptions } from "./application/options.js"
export type { EventHandler } from "./application/events.js"
export {
  permissionTargets,
  permissionTargetConversation,
  type PermissionTarget,
} from "./application/permission-target.js"
export {
  interactionDetails,
  questionDetails,
  reviewDetails,
  REVIEW_RETRY_FIRST_MS,
  REVIEW_RETRY_MOST_MS,
  type InteractionDetail,
  type QuestionDetail,
  type QuestionDetails,
  type ReviewDetail,
  type ReviewDetails,
  type ReviewDetailsTimer,
} from "./application/review-details.js"

export {
  captureQuestionChoices,
  questionTargets,
  questionTargetConversation,
  type QuestionTarget,
  type QuestionChoice,
} from "./application/question-target.js"
export type { ClientEventMap } from "./protocol/types.js"
export type { ResolvedConnectRetryOptions } from "./application/connect-retry.js"
export type {
  ResolvedConnectOptions,
  ResolvedProductConnectOptions,
} from "./application/resolve-options.js"
export type {
  ProductGrant,
  ProductCredentialMetadata,
  CredentialIssueParams,
  IssuedCredentialResult,
  ExistingCredentialResult,
  ProductClientMetadata,
  ProductSurface,
  ProductSurfaceKind,
  CredentialListResult,
  CredentialRevokeResult,
} from "./generated/product.js"

export type { SessionTermination } from "./generated/product.js"
export { RecordReadErrorCode } from "./generated/product.js"
export {
  channelRules,
  mcpAppCallTiming,
  productMethodGrants,
} from "./generated/product.js"
export type {
  ConversationRecordsHeadResult,
  RecordScope,
  RecordPageRequest,
} from "./generated/product.js"
export type { DecodedRecord, DecodedRecordPage } from "./protocol/record-read-validate.js"
export {
  ConversationSubscriptionEndReason,
  ConversationSubscriptionErrorCode,
  subscriptionLimits,
} from "./generated/product.js"
export type { ConversationViewCursor } from "./generated/product.js"

export { NessaMutationError } from "./application/mutation-error.js"
export type {
  ProductMembership,
  ProductPrincipal,
  ProductResource,
} from "./generated/product.js"

export {
  NessaCredentialUnavailableError,
  type CredentialSource,
} from "./application/credential-source.js"
export { LocalFileCredentialSource } from "./transport/local-credential-source.js"

export type {
  ConversationApi,
  ConversationActionOptions,
  ConversationCreateOptions,
  ConversationListOptions,
  ConversationPrepareOptions,
  PreparedSubmission,
  SubmissionInput,
  ConversationShareOptions,
  ConversationSubmission,
} from "./presentation/conversation-api.js"
export type {
  AttachmentApi,
  AttachmentBeginning,
  AttachmentDescription,
} from "./presentation/attachment-api.js"
export {
  NessaAttachmentError,
  type AttachmentBeginRefusal,
  type AttachmentFailureCode,
} from "./application/attachment-upload.js"
export {
  HELD_FILE_BUSY_RETRY_DELAYS_MS,
  HELD_FILE_RANGE_BYTES,
  NessaHeldFileError,
  type HeldFileFailureCode,
} from "./application/held-file-read.js"
export {
  mcpAppDeadlines,
  type McpAppsApi,
  type McpResourceDescription,
} from "./presentation/mcp-apps-api.js"
export { NessaMcpAppError } from "./application/mcp-app-call.js"
export {
  carriesMcpServersGrant,
  type McpServersApi,
} from "./presentation/mcp-servers-api.js"
export { type GithubApi } from "./presentation/github-api.js"
export {
  GithubRefusalCode,
  NessaGithubError,
  type GithubMethod,
} from "./application/github-error.js"
export {
  GithubConnectEnd,
  GithubConnectState,
  GithubErrorCode,
  type GithubAccount,
  type GithubCode,
  type GithubConnection,
} from "./generated/product.js"
export { type PairingApi } from "./presentation/pairing-api.js"
export {
  NessaPairingError,
  PairingRefusalCode,
  type PairingMethod,
} from "./application/pairing-error.js"
export {
  NessaMcpServersError,
  type McpServersMethod,
  type McpServersRefusal,
} from "./application/mcp-servers-error.js"
export {
  McpServerKind,
  McpServerProblemCode,
  McpServersErrorCode,
  McpServersInspectCut,
  mcpServerInspect,
} from "./generated/product.js"
export type {
  McpInspectedTool,
  McpInspectedUi,
  McpServerEnvEntry,
  McpServerInput,
  McpServerListEntry,
  McpServersAuditUnavailableDetails,
  McpServersConfigTooLargeDetails,
  McpServersInspectResult,
  McpServersInvalidDetails,
  McpServersListResult,
  McpServersRemoveParams,
  McpServersRevisionConflictDetails,
  McpServersSaveParams,
  McpServersStorageUnavailableDetails,
  McpServersWriteResult,
} from "./generated/product.js"
export { NessaRequestTooLargeError } from "./application/request-too-large-error.js"
export {
  NessaMcpResourceError,
  type McpResourceFailureCode,
} from "./application/mcp-resource-fetch.js"
export {
  MAX_MCP_ARGUMENTS_BYTES,
  MAX_MCP_CONTEXT_BYTES,
  MAX_MCP_MESSAGE_BYTES,
  MAX_MCP_RESOURCE_BYTES,
  MAX_MCP_RESULT_BYTES,
  mcpAppRequestProblem,
  type McpAppModelContext,
} from "./protocol/mcp-app-validate.js"
export type {
  McpAppReference,
  McpCallToolResult,
  McpReadResourceResult,
  McpRemoteErrorDetails,
  McpSendMessageResult,
  McpUiCsp,
  McpUiPermissions,
} from "./generated/product.js"
export {
  asImageAttachment,
  imageAttachmentsProblem,
  linkedFileProblem,
  linkedFilesProblem,
  IMAGE_ATTACHMENT_TYPES,
  MAX_FILE_PATH_BYTES,
  MAX_IMAGE_ATTACHMENT_BYTES,
  MAX_MESSAGE_FILES,
  MAX_MESSAGE_IMAGE_BYTES,
  MAX_MESSAGE_IMAGES,
  MAX_UPLOAD_BYTES,
  type StoredAttachment,
} from "./protocol/attachment-validate.js"
export { clientDueAtMs, deadlinePendingLine } from "./protocol/deadline-pending.js"
export type {
  ConversationView,
  ApprovalMode,
  ApprovalModeChoice,
  AgentModelOption,
  AgentOption,
  NativeHooksAgent,
  AgentsSetNativeHooksResult,
  InstallableAgent,
  AgentInstallOffer,
  AgentInstallOptionsResult,
  AgentInstallResult,
  AgentsListResult,
  ConversationSetApprovalModeResult,
  ConversationListResult,
  ConversationObserveCursor,
  ConversationObserveResult,
  ConversationSummary,
  ConversationLifecycle,
  ConversationLifecyclePhase,
  ConversationStartupFailure,
  ConversationStartupFailureCode,
  ConversationAttachmentEvidenceFailure,
  ImageAttachment,
  LinkedFile,
  ConversationMessage,
  ConversationMessageApp,
  ConversationPart,
  ConversationRuntime,
  ConversationPending,
  ConversationPermission,
  ConversationQuestion,
  ConversationQuestionEntry,
  ConversationReviewEntry,
  ConversationPermissionOption,
  ConversationTool,
  ConversationToolChange,
  ConversationCapabilities,
  ConversationAgentFeatures,
  PermissionDenialSupport,
  NativeHookSuppressionSupport,
  CompactionReportingSupport,
  ModelSwitchReportingSupport,
  PermissionDeferralSupport,
  ElicitationForwardingSupport,
  PreToolPolicySupport,
  PolicyEndTurnSupport,
  PolicyCloseSessionSupport,
  IncomingElicitationSupport,
  ConversationMessageStatus,
  ConversationDisposition,
  ConversationPendingMode,
  ConversationMutationResult,
  ConversationReorderResult,
  ConversationReorderOutcome,
  ConversationCreateResult,
  ConversationReceipt,
  ConversationPermissionSelectionState,
  PermissionRule,
  PermissionRuleEffect,
  ConversationPermissionRulesResult,
  ConversationRevokePermissionRuleResult,
  PermissionReviewNotice,
  PermissionReviewWaiting,
  ConversationShare,
  ConversationShareResult,
  ConversationSharesCursor,
  ConversationSharesResult,
  ConversationToolPolicy,
} from "./generated/product.js"
export {
  ConversationErrorCode,
  ConversationCapability,
  ConversationToolPreset,
} from "./generated/product.js"
export {
  sharePage,
  toolPolicyBounds,
  viewEntryBounds,
} from "./protocol/conversation-validate.js"
export { conversationErrorCode } from "./application/conversation-error-code.js"
export type { ConversationLease } from "./generated/product.js"
export {
  conversationLeasePlace,
  environmentPlace,
  isPairedEnvironment,
  conversationLeaseStatus,
} from "./application/conversation-lease.js"
export {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "./application/conversation-mutation-error.js"

export { isRetryableConnectionError } from "./application/connect-retry.js"

export type { CatalogueReadApi } from "./presentation/catalogue-read-api.js"
export type { DecodedCatalogueResolve } from "./protocol/catalogue-read-validate.js"
export type {
  CataloguePass,
  CatalogueDescriptor,
  CatalogueEntryKey,
  CatalogueManifestRequest,
  ConversationCatalogueHeadParams,
  ConversationCatalogueHeadResult,
  ConversationCatalogueManifestParams,
  ConversationCatalogueManifestResult,
  ConversationCatalogueResolveParams,
  ConversationCatalogueResolveResult,
} from "./generated/product.js"
export { CatalogueReadErrorCode } from "./generated/product.js"

export {
  conversationTitle,
  prepareSubmission,
  preparedSubmissionFields,
} from "./presentation/conversation-api.js"
export {
  executionTargets,
  executionTargetConversation,
  type ExecutionTarget,
} from "./application/execution-target.js"
export type {
  SendMessageOptions,
  ThreadMessageMode,
  ThreadMessageReceipt,
  StopThreadOptions,
  StopThreadResult,
} from "./presentation/thread-api.js"
export {
  NessaThreadSubscriptionError,
  type ThreadEventBatch,
  type ThreadSubscribeOptions,
  type ThreadSubscription,
} from "./presentation/thread-subscription.js"
export type {
  ConversationEvent,
  ConversationEventKind,
  ConversationEvents,
  ConversationMessageReceipt,
} from "./generated/product.js"
export type {
  Channel,
  ChannelCreateParams,
  ChannelListResult,
} from "./generated/product.js"
export { ChannelErrorCode } from "./generated/product.js"
export type {
  WorkspaceArrangement,
  WorkspaceColumn,
  WorkspacePane,
  WorkspacePaneItem,
  WorkspacePaneKind,
  WorkspacePublishResult,
  WorkspaceShowing,
  WorkspaceWidget,
  WorkspaceWindow,
} from "./generated/product.js"
