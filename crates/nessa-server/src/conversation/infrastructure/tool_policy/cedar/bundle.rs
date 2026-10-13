//! The checked-in schema and policies, loaded and strictly validated once.

use cedar_policy::{Policy, PolicyId, PolicySet, Schema, ValidationMode, Validator};
use std::str::FromStr;

use crate::conversation::application::ToolPolicyUnavailable;

use super::grant_rules::GRANT_RULES;

const SCHEMA: &str = include_str!("schema.json");

/// Every policy file in the bundle. A new policy set adds its file here.
const POLICY_FILES: &[&str] = &[include_str!("tools.cedar")];

/// The id prefixes of policies built at evaluation from stored data, one per
/// policy set that builds them. No checked-in policy uses one, so a built
/// policy's id never collides with a checked-in one.
const BUILT: &[&str] = &[GRANT_RULES];

/// The bundle: its schema, and its checked-in policies under their `@id`s.
pub(super) struct Bundle {
    schema: Schema,
    policies: PolicySet,
}

impl Bundle {
    /// The checked-in bundle, refused when the schema or a policy file does
    /// not parse, a policy has no `@id`, an `@id` repeats or is reserved for
    /// policies built from stored data, or the policies do not validate
    /// strictly against the schema without a warning.
    pub(super) fn load() -> Result<Self, ToolPolicyUnavailable> {
        Self::from_sources(SCHEMA, POLICY_FILES)
    }

    pub(super) fn from_sources(
        schema: &str,
        files: &[&str],
    ) -> Result<Self, ToolPolicyUnavailable> {
        let schema = Schema::from_json_str(schema).map_err(|_| ToolPolicyUnavailable)?;
        let mut named = Vec::new();
        for file in files {
            let parsed = PolicySet::from_str(file).map_err(|_| ToolPolicyUnavailable)?;
            for policy in parsed.policies() {
                let id = policy.annotation("id").ok_or(ToolPolicyUnavailable)?;
                if BUILT.iter().any(|prefix| id.starts_with(prefix)) {
                    return Err(ToolPolicyUnavailable);
                }
                named.push(policy.new_id(PolicyId::new(id)));
            }
        }
        let bundle = Self {
            schema,
            policies: PolicySet::new(),
        };
        let policies = bundle.with(named)?;
        Ok(Self { policies, ..bundle })
    }

    pub(super) fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The bundle's policies with `extra` added, validated together. The
    /// whole set is validated on each call; a caller that evaluates one
    /// grant's rules often may keep the result for that grant and revision.
    pub(super) fn with(&self, extra: Vec<Policy>) -> Result<PolicySet, ToolPolicyUnavailable> {
        let mut policies = self.policies.clone();
        for policy in extra {
            policies.add(policy).map_err(|_| ToolPolicyUnavailable)?;
        }
        let validation =
            Validator::new(self.schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            return Err(ToolPolicyUnavailable);
        }
        Ok(policies)
    }

    /// The checked-in policies alone.
    pub(super) fn policies(&self) -> &PolicySet {
        &self.policies
    }
}
