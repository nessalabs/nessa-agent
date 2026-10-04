//! Private core facades share the admitted session and retain one typed outcome.
use super::session::{RpcKind, Session};
use crate::product_contract::generated::{CatalogueReadErrorCode, RecordReadErrorCode};
use crate::{
    conversation::{
        application::{
            validate_catalogue_selector, validate_record_selector, CatalogueReadScope,
            ReceiverReadScope,
        },
        domain::ConversationId,
        infrastructure::NessaCatalogueSource,
    },
    product::{
        catalogue_read::wire as catalogue_wire,
        generated::{
            product_method, CatalogueManifestRequest, ConversationCatalogueHeadParams,
            ConversationCatalogueHeadResult, ConversationCatalogueManifestParams,
            ConversationCatalogueManifestResult, ConversationCatalogueResolveParams,
            ConversationCatalogueResolveResult, ConversationRecordsHeadParams,
            ConversationRecordsHeadResult, ConversationRecordsPageParams,
            ConversationRecordsPageResult, ConversationWatchRecordsParams,
        },
        passive_read::wire::ReadWireError,
        record_read::wire as record_wire,
    },
    read_only_sync::application::{
        watch::{Registered, Wait},
        GatewayAttempt, GatewayError, GatewayOutcome,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::infrastructure::session_storage::physical_record_schema;
use nessa_sync::replication::{
    application::{Access, RecordSource, ScopeAuthorizer, SourceError},
    catalogue::{
        validate_catalogue_pass, validate_manifest, validate_manifest_request, validate_resolved,
        CataloguePass, CatalogueSource, CatalogueSourceError, ManifestEntry, ManifestPage,
        ManifestRequest, ResolvedEntry, MAX_CATALOGUE_ENTRIES,
    },
    domain::{Id, Page, PageRequest, Scope},
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    cell::RefCell,
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

#[derive(Clone)]
enum Target {
    Record(ConversationId),
    Catalogue,
}
#[derive(Clone)]
struct GatewaySource {
    session: Rc<RefCell<Session>>,
    receiver: Id,
    epoch: u64,
    target: Target,
    descriptors: Option<(CataloguePass, Vec<ManifestEntry>)>,
}
/// Factory publishes paired facades, never a second connection/admission owner.
pub(crate) struct GatewayConnection(Rc<RefCell<Session>>);
impl GatewayConnection {
    pub(crate) fn new(session: Session) -> Self {
        Self(Rc::new(RefCell::new(session)))
    }
    pub(crate) fn begin(&self) -> Result<u64, GatewayError> {
        self.0
            .try_borrow_mut()
            .map_err(|_| GatewayError::Busy)?
            .begin()
    }
    pub(crate) fn finish(&self) -> Result<GatewayOutcome, GatewayError> {
        self.0
            .try_borrow_mut()
            .map_err(|_| GatewayError::Busy)?
            .finish()
    }
    /// The calling thread owns callback completion and socket failure cleanup.
    pub(crate) fn run<R>(
        &self,
        execute: impl FnOnce() -> R,
    ) -> Result<GatewayAttempt<R>, GatewayError> {
        self.begin()?;
        let result = match catch_unwind(AssertUnwindSafe(execute)) {
            Ok(value) => Some(value),
            Err(_) => {
                self.0.borrow_mut().fail(GatewayError::DriverPanicked);
                None
            }
        };
        Ok(GatewayAttempt {
            result,
            outcome: self.finish()?,
        })
    }
    /// Register the connection's one record watch as its own operation.
    pub(crate) fn watch_records(
        &self,
        receiver: &Id,
        epoch: u64,
        conversation: &ConversationId,
    ) -> Result<Registered, GatewayError> {
        let params = ConversationWatchRecordsParams {
            conversation_id: conversation.to_string(),
            receiver_id: receiver.as_str().to_owned(),
            access_epoch: epoch.to_string(),
        };
        let attempt = self.run(|| {
            self.0
                .try_borrow_mut()
                .map_err(|_| GatewayError::Busy)?
                .watch_records(&params)
        })?;
        let operation = attempt.outcome.operation;
        settled(attempt).map(|watch| Registered { watch, operation })
    }
    /// Wait for the watch's next hint or end as its own operation.
    pub(crate) fn wait_hint(&self) -> Result<Wait, GatewayError> {
        settled(self.run(|| {
            self.0
                .try_borrow_mut()
                .map_err(|_| GatewayError::Busy)?
                .wait_hint()
        })?)
    }
    pub(crate) fn records(
        &self,
        receiver: Id,
        epoch: u64,
        conversation: ConversationId,
    ) -> RecordGatewaySource {
        RecordGatewaySource(GatewaySource {
            session: self.0.clone(),
            receiver,
            epoch,
            target: Target::Record(conversation),
            descriptors: None,
        })
    }
    pub(crate) fn catalogue(&self, receiver: Id, epoch: u64) -> CatalogueGatewaySource {
        CatalogueGatewaySource(GatewaySource {
            session: self.0.clone(),
            receiver,
            epoch,
            target: Target::Catalogue,
            descriptors: None,
        })
    }
}
/// One operation's value, or the first failure the operation retained.
fn settled<R>(attempt: GatewayAttempt<Result<R, GatewayError>>) -> Result<R, GatewayError> {
    match (attempt.result, attempt.outcome.failure) {
        (_, Some(failure)) => Err(failure),
        (Some(result), None) => result,
        (None, None) => Err(GatewayError::DriverPanicked),
    }
}
impl GatewaySource {
    fn authorizer(&self) -> GatewayAuthorizer {
        GatewayAuthorizer(GatewaySource {
            session: self.session.clone(),
            receiver: self.receiver.clone(),
            epoch: self.epoch,
            target: self.target.clone(),
            descriptors: None,
        })
    }
    pub(crate) fn discover(&mut self) -> Result<(Scope, u64), GatewayError> {
        let result = self.discover_inner();
        result.map_err(|error| self.fail(error))
    }
    fn fail(&self, error: GatewayError) -> GatewayError {
        if let Ok(mut session) = self.session.try_borrow_mut() {
            session.fail(error);
        }
        error
    }
    fn rpc<T: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &T,
        kind: RpcKind,
    ) -> Result<R, GatewayError> {
        let value = self
            .session
            .try_borrow_mut()
            .map_err(|_| GatewayError::Busy)?
            .rpc(method, params, kind)?;
        serde_json::from_value(value).map_err(|_| self.fail(GatewayError::Protocol))
    }
    fn discover_inner(&self) -> Result<(Scope, u64), GatewayError> {
        let (scope, head) = match &self.target {
            Target::Record(conversation) => {
                let wire: ConversationRecordsHeadResult = self.rpc(
                    product_method::CONVERSATION_RECORDS_HEAD,
                    &ConversationRecordsHeadParams {
                        conversation_id: conversation.to_string(),
                        receiver_id: self.receiver.as_str().to_owned(),
                        access_epoch: self.epoch.to_string(),
                    },
                    RpcKind::Record,
                )?;
                record_wire::decode_head(wire).map_err(wire_error)?
            }
            Target::Catalogue => {
                let wire: ConversationCatalogueHeadResult = self.rpc(
                    product_method::CONVERSATION_CATALOGUE_HEAD,
                    &ConversationCatalogueHeadParams {
                        receiver_id: self.receiver.as_str().to_owned(),
                        access_epoch: self.epoch.to_string(),
                    },
                    RpcKind::Catalogue,
                )?;
                catalogue_wire::decode_head(wire).map_err(wire_error)?
            }
        };
        self.check_scope(&scope)?;
        Ok((scope, head))
    }
    fn check_scope(&self, scope: &Scope) -> Result<(), GatewayError> {
        let session = self.session.try_borrow().map_err(|_| GatewayError::Busy)?;
        let ready = session.ready();
        let organization_id =
            OrganizationId::new(&ready.organization_id).map_err(|_| GatewayError::Protocol)?;
        let owner_id = PrincipalId::new(&ready.principal_id).map_err(|_| GatewayError::Protocol)?;
        if scope.origin().as_str() != ready.gateway_id {
            return Err(GatewayError::Correlation);
        }
        match &self.target {
            Target::Record(conversation) => {
                let admitted = ReceiverReadScope {
                    receiver_id: self.receiver.as_str().to_owned(),
                    organization_id,
                    owner_id,
                    conversation_id: conversation.clone(),
                    access_epoch: self.epoch,
                };
                validate_record_selector(&admitted, scope)
                    .map_err(|_| GatewayError::Correlation)?;
                if scope.schema() != &physical_record_schema() {
                    return Err(GatewayError::Correlation);
                }
            }
            Target::Catalogue => {
                let admitted = CatalogueReadScope {
                    receiver_id: self.receiver.as_str().to_owned(),
                    organization_id: organization_id.clone(),
                    owner_id: owner_id.clone(),
                    access_epoch: self.epoch,
                };
                validate_catalogue_selector(&admitted, scope)
                    .map_err(|_| GatewayError::Correlation)?;
                NessaCatalogueSource::check_scope_identity(&organization_id, &owner_id, scope)
                    .map_err(|_| GatewayError::Correlation)?;
            }
        }
        Ok(())
    }
}
impl ScopeAuthorizer for GatewaySource {
    fn authorize(&mut self, _requested: &Scope) -> Access {
        match self.discover() {
            Ok((actual, _)) => Access::Allowed(actual),
            Err(
                GatewayError::Record(
                    RecordReadErrorCode::Unauthorized | RecordReadErrorCode::Forbidden,
                )
                | GatewayError::Catalogue(
                    CatalogueReadErrorCode::Unauthorized | CatalogueReadErrorCode::Forbidden,
                ),
            ) => Access::Denied,
            Err(_) => Access::Unverifiable,
        }
    }
}
impl RecordSource for GatewaySource {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        let (actual, head) = self.discover().map_err(record_error)?;
        if &actual != scope {
            self.fail(GatewayError::ScopeChanged);
            return Err(SourceError::IdentityChanged);
        }
        Ok(head)
    }
    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        let result = (|| {
            self.check_scope(&request.scope)?;
            let wire = record_wire::wire_request(request);
            record_wire::decode_page_request(&wire).map_err(|_| GatewayError::InvalidRequest)?;
            let wire: ConversationRecordsPageResult = self.rpc(
                product_method::CONVERSATION_RECORDS_PAGE,
                &ConversationRecordsPageParams {
                    request: wire,
                    conversation_id: match &self.target {
                        Target::Record(conversation) => conversation.to_string(),
                        Target::Catalogue => return Err(GatewayError::Correlation),
                    },
                    access_epoch: self.epoch.to_string(),
                },
                RpcKind::Record,
            )?;
            record_wire::decode_page_result(wire, request).map_err(wire_error)
        })();
        result.map_err(|error| record_error(self.fail(error)))
    }
}
impl CatalogueSource for GatewaySource {
    fn head(&mut self, scope: &Scope) -> Result<u64, CatalogueSourceError> {
        let (actual, head) = self.discover().map_err(catalogue_error)?;
        if &actual != scope {
            self.fail(GatewayError::ScopeChanged);
            return Err(CatalogueSourceError::IdentityChanged);
        }
        Ok(head)
    }
    fn manifest(
        &mut self,
        request: &ManifestRequest,
    ) -> Result<ManifestPage, CatalogueSourceError> {
        let result = (|| {
            validate_manifest_request(request, MAX_CATALOGUE_ENTRIES)
                .map_err(|_| GatewayError::InvalidRequest)?;
            self.check_scope(&request.pass.scope)?;
            let wire: ConversationCatalogueManifestResult = self.rpc(
                product_method::CONVERSATION_CATALOGUE_MANIFEST,
                &ConversationCatalogueManifestParams {
                    access_epoch: self.epoch.to_string(),
                    request: CatalogueManifestRequest {
                        pass: catalogue_wire::wire_pass(&request.pass),
                        max_entries: request.max_entries as u64,
                    },
                },
                RpcKind::Catalogue,
            )?;
            let page = catalogue_wire::decode_manifest_result(wire).map_err(wire_error)?;
            validate_manifest(request, &page, MAX_CATALOGUE_ENTRIES)
                .map_err(|_| GatewayError::Protocol)?;
            self.descriptors = Some((request.pass.clone(), page.entries.clone()));
            Ok(page)
        })();
        result.map_err(|error| catalogue_error(self.fail(error)))
    }
    fn resolve(
        &mut self,
        pass: &CataloguePass,
        id: &Id,
        maximum: usize,
    ) -> Result<ResolvedEntry, CatalogueSourceError> {
        let result = (|| {
            validate_catalogue_pass(pass).map_err(|_| GatewayError::InvalidRequest)?;
            self.check_scope(&pass.scope)?;
            let (saved_pass, entries) =
                self.descriptors.as_ref().ok_or(GatewayError::Correlation)?;
            if saved_pass != pass {
                return Err(GatewayError::Correlation);
            }
            let descriptor = entries
                .iter()
                .find(|entry| &entry.key.id == id)
                .ok_or(GatewayError::Correlation)?;
            let wire: ConversationCatalogueResolveResult = self.rpc(
                product_method::CONVERSATION_CATALOGUE_RESOLVE,
                &ConversationCatalogueResolveParams {
                    access_epoch: self.epoch.to_string(),
                    pass: catalogue_wire::wire_pass(pass),
                    descriptor: catalogue_wire::wire_descriptor(descriptor),
                    max_payload_bytes: maximum as u64,
                },
                RpcKind::Catalogue,
            )?;
            let (actual_pass, actual_descriptor, entry) =
                catalogue_wire::decode_resolved_result(wire, maximum).map_err(wire_error)?;
            if actual_pass != *pass || actual_descriptor != *descriptor {
                return Err(GatewayError::Correlation);
            }
            validate_resolved(descriptor, &entry, maximum).map_err(|_| GatewayError::Protocol)?;
            Ok(entry)
        })();
        result.map_err(|error| catalogue_error(self.fail(error)))
    }
}
fn wire_error(error: ReadWireError) -> GatewayError {
    match error {
        ReadWireError::ResponseTooLarge => GatewayError::ResponseTooLarge,
        _ => GatewayError::Protocol,
    }
}
fn record_error(error: GatewayError) -> SourceError {
    match error {
        GatewayError::ScopeChanged | GatewayError::Record(RecordReadErrorCode::IdentityChanged) => {
            SourceError::IdentityChanged
        }
        GatewayError::Record(RecordReadErrorCode::HistoryPruned) => SourceError::Pruned,
        GatewayError::Record(RecordReadErrorCode::RecordTooLarge) => SourceError::OversizedRecord,
        GatewayError::InvalidRequest
        | GatewayError::Record(RecordReadErrorCode::InvalidRequest) => SourceError::InvalidRequest,
        _ => SourceError::Unavailable,
    }
}
fn catalogue_error(error: GatewayError) -> CatalogueSourceError {
    match error {
        GatewayError::ScopeChanged
        | GatewayError::Catalogue(CatalogueReadErrorCode::IdentityChanged) => {
            CatalogueSourceError::IdentityChanged
        }
        GatewayError::Catalogue(CatalogueReadErrorCode::OversizedEntry) => {
            CatalogueSourceError::OversizedEntry
        }
        GatewayError::InvalidRequest
        | GatewayError::Catalogue(CatalogueReadErrorCode::InvalidRequest) => {
            CatalogueSourceError::InvalidRequest
        }
        _ => CatalogueSourceError::Unavailable,
    }
}

// Separate facades constrain which source port a target can become.
pub(crate) struct RecordGatewaySource(GatewaySource);
pub(crate) struct CatalogueGatewaySource(GatewaySource);
pub(crate) struct GatewayAuthorizer(GatewaySource);
impl RecordGatewaySource {
    pub(crate) fn discover(&mut self) -> Result<(Scope, u64), GatewayError> {
        self.0.discover()
    }
    pub(crate) fn authorizer(&self) -> GatewayAuthorizer {
        self.0.authorizer()
    }
}
impl CatalogueGatewaySource {
    pub(crate) fn discover(&mut self) -> Result<(Scope, u64), GatewayError> {
        self.0.discover()
    }
    pub(crate) fn authorizer(&self) -> GatewayAuthorizer {
        self.0.authorizer()
    }
}
impl ScopeAuthorizer for GatewayAuthorizer {
    fn authorize(&mut self, scope: &Scope) -> Access {
        self.0.authorize(scope)
    }
}
impl RecordSource for RecordGatewaySource {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        RecordSource::head(&mut self.0, scope)
    }
    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        self.0.page(request)
    }
}
impl CatalogueSource for CatalogueGatewaySource {
    fn head(&mut self, scope: &Scope) -> Result<u64, CatalogueSourceError> {
        CatalogueSource::head(&mut self.0, scope)
    }
    fn manifest(
        &mut self,
        request: &ManifestRequest,
    ) -> Result<ManifestPage, CatalogueSourceError> {
        self.0.manifest(request)
    }
    fn resolve(
        &mut self,
        pass: &CataloguePass,
        id: &Id,
        maximum: usize,
    ) -> Result<ResolvedEntry, CatalogueSourceError> {
        self.0.resolve(pass, id, maximum)
    }
}
