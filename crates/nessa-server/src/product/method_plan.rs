//! Exhaustive identity classification for the product gateway.
//!
//! The protocol owns wire names; this module owns static route, response lane,
//! execution capacity and admission shape. It contains no authorization verdict
//! or runtime permit. Current policy and physical work stay with their owners.
use nessa_protocol::product::generated::ProductMethod;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handler {
    Authentication,
    CurrentSession,
    Health,
    CredentialIssue,
    CredentialList,
    CredentialRevoke,
    AgentsList,
    NativeHooks,
    AgentInstall,
    Conversation,
    Attachment,
    ArtifactRead,
    McpApps,
    McpServers,
    Pairing,
    Peers,
    RecordRead,
    CatalogueRead,
    Share,
    Watch,
    Subscription,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponseLane {
    Control,
    Ordinary,
    Record,
    App,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecutionCapacity {
    Controls,
    RecordReads,
    Deletions,
    UploadBegins,
    ArtifactReads,
    Requests,
    AppOwned,
    ConnectionOwned,
    Authentication,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestAccess {
    Drive,
    Comment,
    CatalogueWrite,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShareAction {
    Share,
    Unshare,
    Shares,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionPlan {
    Authentication,
    CurrentSession,
    Manifest,
    Request(RequestAccess),
    ManifestThenConversationRead,
    ManifestThenList,
    ManifestThenArtifactRead,
    PassiveRecord,
    PassiveCatalogue,
    ManifestThenShare(ShareAction),
    WatchRecords,
    WatchCatalogue,
    Unwatch,
    FollowConversation,
    FollowList,
    FollowAgents,
    UnsubscribeConversation,
    UnsubscribeAgents,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MethodPlan {
    pub(super) handler: Handler,
    pub(super) response: ResponseLane,
    pub(super) capacity: ExecutionCapacity,
    pub(super) admission: AdmissionPlan,
}
/// Classify every known method explicitly. Unknown wire strings stay outside
/// this function so their bounded refusal path is not a default for new methods.
pub(super) const fn method_plan(method: ProductMethod) -> MethodPlan {
    match method {
        ProductMethod::SessionAuthenticate => MethodPlan {
            handler: Handler::Authentication,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Authentication,
            admission: AdmissionPlan::Authentication,
        },
        ProductMethod::AuthSession => MethodPlan {
            handler: Handler::CurrentSession,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::CurrentSession,
        },
        ProductMethod::ServerHealth => MethodPlan {
            handler: Handler::Health,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::CredentialIssue => MethodPlan {
            handler: Handler::CredentialIssue,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::CredentialList => MethodPlan {
            handler: Handler::CredentialList,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::CredentialRevoke => MethodPlan {
            handler: Handler::CredentialRevoke,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::ConversationCreate => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationRead => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::ManifestThenConversationRead,
        },
        ProductMethod::ConversationRecordsHead => MethodPlan {
            handler: Handler::RecordRead,
            response: ResponseLane::Record,
            capacity: ExecutionCapacity::RecordReads,
            admission: AdmissionPlan::PassiveRecord,
        },
        ProductMethod::ConversationRecordsPage => MethodPlan {
            handler: Handler::RecordRead,
            response: ResponseLane::Record,
            capacity: ExecutionCapacity::RecordReads,
            admission: AdmissionPlan::PassiveRecord,
        },
        ProductMethod::ConversationList => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::ManifestThenList,
        },
        ProductMethod::ConversationObserve => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::ManifestThenList,
        },
        ProductMethod::ConversationSend => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationSteer => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationRemove => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationStop => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationReceipt => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationMessage => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::Comment),
        },
        ProductMethod::ConversationMessageStatus => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::Comment),
        },
        ProductMethod::ConversationAnswer => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationAnswerQuestion => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationCancel => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::Drive),
        },
        ProductMethod::ConversationClose => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationArchive => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationUnarchive => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationDelete => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Deletions,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationReorder => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ArtifactRead => MethodPlan {
            handler: Handler::ArtifactRead,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ArtifactReads,
            admission: AdmissionPlan::ManifestThenArtifactRead,
        },
        ProductMethod::AttachmentBegin => MethodPlan {
            handler: Handler::Attachment,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::UploadBegins,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::AgentsList => MethodPlan {
            handler: Handler::AgentsList,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::AgentsSetNativeHooks => MethodPlan {
            handler: Handler::NativeHooks,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::AgentsSubscribe => MethodPlan {
            handler: Handler::Subscription,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::FollowAgents,
        },
        ProductMethod::AgentsUnsubscribe => MethodPlan {
            handler: Handler::Subscription,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::UnsubscribeAgents,
        },
        ProductMethod::ConversationSetApprovalMode => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::AgentsInstallOptions => MethodPlan {
            handler: Handler::AgentInstall,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::AgentsInstall => MethodPlan {
            handler: Handler::AgentInstall,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationCatalogueHead => MethodPlan {
            handler: Handler::CatalogueRead,
            response: ResponseLane::Record,
            capacity: ExecutionCapacity::RecordReads,
            admission: AdmissionPlan::PassiveCatalogue,
        },
        ProductMethod::ConversationCatalogueManifest => MethodPlan {
            handler: Handler::CatalogueRead,
            response: ResponseLane::Record,
            capacity: ExecutionCapacity::RecordReads,
            admission: AdmissionPlan::PassiveCatalogue,
        },
        ProductMethod::ConversationCatalogueResolve => MethodPlan {
            handler: Handler::CatalogueRead,
            response: ResponseLane::Record,
            capacity: ExecutionCapacity::RecordReads,
            admission: AdmissionPlan::PassiveCatalogue,
        },
        ProductMethod::McpCallTool => MethodPlan {
            handler: Handler::McpApps,
            response: ResponseLane::App,
            capacity: ExecutionCapacity::AppOwned,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::McpReadResource => MethodPlan {
            handler: Handler::McpApps,
            response: ResponseLane::App,
            capacity: ExecutionCapacity::AppOwned,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::McpReleaseApp => MethodPlan {
            handler: Handler::McpApps,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::McpSendMessage => MethodPlan {
            handler: Handler::McpApps,
            response: ResponseLane::App,
            capacity: ExecutionCapacity::AppOwned,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::McpUpdateModelContext => MethodPlan {
            handler: Handler::McpApps,
            response: ResponseLane::App,
            capacity: ExecutionCapacity::AppOwned,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::McpServersList => MethodPlan {
            handler: Handler::McpServers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::McpServersSave => MethodPlan {
            handler: Handler::McpServers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::McpServersRemove => MethodPlan {
            handler: Handler::McpServers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::McpServersInspect => MethodPlan {
            handler: Handler::McpServers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::McpServersAuthorize => MethodPlan {
            handler: Handler::McpServers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::McpServersRevoke => MethodPlan {
            handler: Handler::McpServers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PairingCreate => MethodPlan {
            handler: Handler::Pairing,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PairingPending => MethodPlan {
            handler: Handler::Pairing,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PairingStatus => MethodPlan {
            handler: Handler::Pairing,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PairingApprove => MethodPlan {
            handler: Handler::Pairing,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PairingDeny => MethodPlan {
            handler: Handler::Pairing,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PairingCancel => MethodPlan {
            handler: Handler::Pairing,
            response: ResponseLane::Control,
            capacity: ExecutionCapacity::Controls,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PeerEnroll => MethodPlan {
            handler: Handler::Peers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PeerList => MethodPlan {
            handler: Handler::Peers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PeerForget => MethodPlan {
            handler: Handler::Peers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::PeerRelay => MethodPlan {
            handler: Handler::Peers,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Manifest,
        },
        ProductMethod::ConversationWatchRecords => MethodPlan {
            handler: Handler::Watch,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::WatchRecords,
        },
        ProductMethod::ConversationWatchCatalogue => MethodPlan {
            handler: Handler::Watch,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::WatchCatalogue,
        },
        ProductMethod::ConversationUnwatch => MethodPlan {
            handler: Handler::Watch,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::Unwatch,
        },
        ProductMethod::ConversationSubscribe => MethodPlan {
            handler: Handler::Subscription,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::FollowConversation,
        },
        ProductMethod::ConversationSubscribeList => MethodPlan {
            handler: Handler::Subscription,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::FollowList,
        },
        ProductMethod::ConversationUnsubscribe => MethodPlan {
            handler: Handler::Subscription,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::ConnectionOwned,
            admission: AdmissionPlan::UnsubscribeConversation,
        },
        ProductMethod::ConversationShare => MethodPlan {
            handler: Handler::Share,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::ManifestThenShare(ShareAction::Share),
        },
        ProductMethod::ConversationUnshare => MethodPlan {
            handler: Handler::Share,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::ManifestThenShare(ShareAction::Unshare),
        },
        ProductMethod::ConversationShares => MethodPlan {
            handler: Handler::Share,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::ManifestThenShare(ShareAction::Shares),
        },
        ProductMethod::ConversationPermissionRules => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
        ProductMethod::ConversationRevokePermissionRule => MethodPlan {
            handler: Handler::Conversation,
            response: ResponseLane::Ordinary,
            capacity: ExecutionCapacity::Requests,
            admission: AdmissionPlan::Request(RequestAccess::CatalogueWrite),
        },
    }
}
#[cfg(test)]
#[path = "../../tests/product/method_plan.rs"]
mod tests;
