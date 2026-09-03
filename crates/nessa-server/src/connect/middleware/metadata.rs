use crate::protocol::{ConnectParams, ResponseFrame};

/// Reject connect params that violate schema string constraints.
pub fn validate_connect_metadata(
    request_id: &str,
    params: &ConnectParams,
) -> Option<ResponseFrame> {
    if params.surface.instance.is_empty() {
        return Some(invalid_params(
            request_id,
            "surface.instance must not be empty",
        ));
    }
    if params.client.id.is_empty() {
        return Some(invalid_params(request_id, "client.id must not be empty"));
    }
    if params.client.version.is_empty() {
        return Some(invalid_params(
            request_id,
            "client.version must not be empty",
        ));
    }
    if params.client.platform.is_empty() {
        return Some(invalid_params(
            request_id,
            "client.platform must not be empty",
        ));
    }
    if params.auth.token.is_empty() {
        return Some(invalid_params(request_id, "auth.token must not be empty"));
    }
    if params.auth.nonce.is_empty() {
        return Some(invalid_params(request_id, "auth.nonce must not be empty"));
    }

    None
}

fn invalid_params(request_id: &str, message: &str) -> ResponseFrame {
    ResponseFrame::failure(request_id, "invalid_params", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AuthToken, ClientInfo, ClientRole, SurfaceInfo, SurfaceKind};

    fn params(instance: &str) -> ConnectParams {
        ConnectParams {
            min_protocol: 1,
            max_protocol: 1,
            role: ClientRole::Surface,
            surface: SurfaceInfo {
                kind: SurfaceKind::Panel,
                instance: instance.to_string(),
            },
            client: ClientInfo {
                id: "client".to_string(),
                version: "0.1.0".to_string(),
                platform: "test".to_string(),
            },
            auth: AuthToken {
                token: "token".to_string(),
                nonce: "nonce".to_string(),
            },
        }
    }

    #[test]
    fn rejects_empty_surface_instance() {
        let error = validate_connect_metadata("1", &params(""))
            .expect("invalid params")
            .error
            .expect("error");
        assert_eq!(error.code, "invalid_params");
    }
}
