use crate::cli::application::{
    issue_token, BrowserToken, CliError, Gateway, GatewayIdentity, TokenRequest, MAX_SAFE_TIMESTAMP,
};
struct Fake {
    identity: GatewayIdentity,
    requests: Vec<TokenRequest>,
    fail: bool,
}
impl Gateway for Fake {
    fn identity(&self) -> &GatewayIdentity {
        &self.identity
    }
    fn health(&mut self) -> Result<(), CliError> {
        Ok(())
    }
    fn issue(&mut self, request: TokenRequest) -> Result<BrowserToken, CliError> {
        self.requests.push(request);
        if self.fail {
            Err(CliError::Denied)
        } else {
            Ok(BrowserToken {
                credential_id: "issued".into(),
                secret: "secret".into(),
            })
        }
    }
}
#[test]
fn token_scope_comes_from_authenticated_gateway_and_failed_issuance_is_not_replayed() {
    for expiry in [None, Some(150), Some(99)] {
        let mut fake = Fake {
            identity: GatewayIdentity {
                gateway_id: "gateway".into(),
                organization_id: "org".into(),
                principal_id: "caller".into(),
                expires_at: expiry,
            },
            requests: vec![],
            fail: false,
        };
        let result = issue_token(
            &mut fake,
            "request".into(),
            "browser".into(),
            "membership".into(),
            100,
            None,
        );
        if expiry == Some(99) {
            assert!(result.is_err());
            assert!(fake.requests.is_empty());
            continue;
        }
        assert_eq!(result.unwrap().credential_id, "issued");
        let request = &fake.requests[0];
        assert_eq!(
            (
                &*request.gateway_id,
                &*request.organization_id,
                &*request.principal_id,
                &*request.membership_id,
                &*request.request_id
            ),
            ("gateway", "org", "browser", "membership", "request")
        );
        assert_eq!(request.expires_at, expiry);
        fake.fail = true;
        assert!(issue_token(
            &mut fake,
            "second".into(),
            "browser2".into(),
            "member2".into(),
            100,
            None
        )
        .is_err());
        assert_eq!(fake.requests.len(), 2);
    }
}

#[test]
fn explicit_ttl_is_capped_and_invalid_lifetimes_never_issue() {
    let mut fake = Fake {
        identity: GatewayIdentity {
            gateway_id: "gateway".into(),
            organization_id: "org".into(),
            principal_id: "caller".into(),
            expires_at: Some(200),
        },
        requests: vec![],
        fail: false,
    };
    for (ttl, expected) in [
        (Some(20), Some(120)),
        (Some(1000), Some(200)),
        (None, Some(200)),
    ] {
        issue_token(
            &mut fake,
            "request".into(),
            "browser".into(),
            "member".into(),
            100,
            ttl,
        )
        .unwrap();
        assert_eq!(fake.requests.last().unwrap().expires_at, expected);
    }
    let count = fake.requests.len();
    for ttl in [0, u64::MAX] {
        assert_eq!(
            issue_token(
                &mut fake,
                "request".into(),
                "browser".into(),
                "member".into(),
                100,
                Some(ttl)
            )
            .err(),
            Some(CliError::InvalidLifetime)
        );
    }
    assert_eq!(fake.requests.len(), count);

    fake.identity.expires_at = Some(MAX_SAFE_TIMESTAMP + 1);
    assert_eq!(
        issue_token(
            &mut fake,
            "request".into(),
            "browser".into(),
            "member".into(),
            100,
            None,
        )
        .err(),
        Some(CliError::InvalidLifetime)
    );
    assert_eq!(fake.requests.len(), count);
}
