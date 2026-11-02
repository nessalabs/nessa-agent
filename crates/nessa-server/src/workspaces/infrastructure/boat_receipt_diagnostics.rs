//! Receipt diagnostics contain compiled field names and JSON kinds, never values.
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    CreateBody,
    CreateDecode,
    CreateValidation,
    InfoBody,
    InfoValidation,
    InfoDecode,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::CreateBody => "create_body",
            Self::CreateDecode => "create_decode",
            Self::CreateValidation => "create_validation",
            Self::InfoBody => "info_body",
            Self::InfoValidation => "info_validation",
            Self::InfoDecode => "info_decode",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Diagnostic {
    stage: Stage,
    scope: &'static str,
    field: &'static str,
    problem: &'static str,
    observed_type: &'static str,
}
impl Diagnostic {
    pub(super) fn opaque(
        stage: Stage,
        scope: &'static str,
        field: &'static str,
        problem: &'static str,
    ) -> Self {
        Self {
            stage,
            scope,
            field,
            problem,
            observed_type: "not_collected",
        }
    }

    pub(super) fn field(
        stage: Stage,
        scope: &'static str,
        field: &'static str,
        problem: &'static str,
        value: Option<&Value>,
    ) -> Self {
        let observed_type = if field == "unknown" {
            "not_collected"
        } else {
            match value {
                None => "missing",
                Some(Value::Null) => "null",
                Some(Value::Bool(_)) => "boolean",
                Some(Value::String(_)) => "string",
                Some(Value::Array(_)) => "array",
                Some(Value::Object(_)) => "object",
                Some(Value::Number(number)) if number.is_f64() => "floating_number",
                Some(Value::Number(number)) => match number.as_u64() {
                    Some(value) if u32::try_from(value).is_ok() => "unsigned_u32",
                    Some(_) => "unsigned_wide",
                    None => "signed_integer",
                },
            }
        };
        Self {
            stage,
            scope,
            field,
            problem,
            observed_type,
        }
    }

    pub(super) fn emit(&self) {
        tracing::warn!(
            target: "nessa_server::workspaces::admission",
            stage = self.stage.name(),
            scope = self.scope,
            field = self.field,
            problem = self.problem,
            observed_type = self.observed_type,
            "Boat receipt rejected"
        );
    }
}

#[cfg(test)]
#[path = "../../../tests/workspaces/boat_receipt_diagnostics.rs"]
mod tests;

/// The SDK owns DTO decoding. Error paths are projected to compiled names;
/// neither its error message nor arbitrary map keys cross into diagnostics.
pub(super) fn decode<T: DeserializeOwned>(
    value: &Value,
    stage: Stage,
    sandbox_fields: &'static [&'static str],
) -> Result<T, Diagnostic> {
    serde_path_to_error::deserialize(value).map_err(|error| {
        let mut segments = error.path().iter();
        let (scope, field) = match segments.next() {
            Some(serde_path_to_error::Segment::Map { key }) if key == "sandbox" => {
                let field = match segments.next() {
                    Some(serde_path_to_error::Segment::Map { key }) => sandbox_fields
                        .iter()
                        .copied()
                        .find(|field| *field == key)
                        .unwrap_or("unknown"),
                    _ => "sandbox",
                };
                ("sandbox", field)
            }
            Some(serde_path_to_error::Segment::Map { key }) => (
                "envelope",
                match key.as_str() {
                    "ok" => "ok",
                    "type" => "type",
                    "status" => "status",
                    "ttlSeconds" => "ttlSeconds",
                    "giftLimitNotice" => "giftLimitNotice",
                    _ => "unknown",
                },
            ),
            _ => ("body", "body"),
        };
        let observed = match scope {
            "sandbox" if field == "sandbox" => value.get("sandbox"),
            "sandbox" => value.get("sandbox").and_then(|v| v.get(field)),
            "envelope" => value.get(field),
            _ => Some(value),
        };
        Diagnostic::field(stage, scope, field, "decode", observed)
    })
}
