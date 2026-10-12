//! `artifact.read` over the admitted session: one range per operation, so
//! each has the whole operation deadline, and a busy answer leaves the
//! connection for the same range again. The answer's base64 is decoded here,
//! canonical only; whether it is the range asked is the application's check.
use super::session::RpcKind;
use super::sources::{settled, GatewayConnection};
use crate::read_only_sync::application::{
    held_file::{HeldFileSource, RangeAnswer},
    GatewayError,
};
use crate::read_only_sync::domain::{HeldFile, HeldRange};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_protocol::product::generated::{product_method, ArtifactReadParams, ArtifactReadResult};

impl HeldFileSource for GatewayConnection {
    fn read(&mut self, file: &HeldFile, range: HeldRange) -> Result<RangeAnswer, GatewayError> {
        let params = ArtifactReadParams {
            conversation_id: file.conversation().to_string(),
            digest: file.digest().to_string(),
            mime_type: file.media_type().as_str().to_owned(),
            size: file.size(),
            offset: range.offset(),
            max_bytes: range.max_bytes(),
        };
        let session = self.0.clone();
        settled(self.run(|| {
            let mut session = session.try_borrow_mut().map_err(|_| GatewayError::Busy)?;
            let value = session.rpc(product_method::ARTIFACT_READ, &params, RpcKind::HeldFile)?;
            decode(value).map_err(|error| session.fail(error))
        })?)
    }
}

/// The answer as sent: its offset, and its bytes from standard padded base64
/// in its one canonical spelling. Anything else is not believed.
pub(super) fn decode(value: serde_json::Value) -> Result<RangeAnswer, GatewayError> {
    let result: ArtifactReadResult =
        serde_json::from_value(value).map_err(|_| GatewayError::Protocol)?;
    let bytes = STANDARD
        .decode(&result.data)
        .map_err(|_| GatewayError::Protocol)?;
    Ok(RangeAnswer {
        offset: result.offset,
        bytes,
    })
}
