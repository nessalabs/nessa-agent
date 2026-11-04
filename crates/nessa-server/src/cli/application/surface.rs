//! The manifest methods `nessa call` and `nessa mcp` share.
//!
//! ```text
//! manifest.json ──▶ surface-catalog.generated.json ──▶ prepare / tools
//! v1.json $defs ──┘
//! ```
//! Arrows are reads of the generated catalog and the product schema. A method
//! the catalog excludes is not prepared and is not a tool. Permission is not
//! decided here.
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

const CATALOG_JSON: &str =
    include_str!("../../../../../protocol/product/surface-catalog.generated.json");
const SCHEMA_JSON: &str = include_str!("../../../../../protocol/product/v1.json");

struct Method {
    name: String,
    params: Option<String>,
    grant: Option<String>,
    read_only: bool,
    destructive: bool,
}

struct Catalog {
    methods: Vec<Method>,
    excluded: HashMap<String, String>,
    defs: Map<String, Value>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let document: Value = serde_json::from_str(CATALOG_JSON).expect("surface catalog is JSON");
        let schema: Value = serde_json::from_str(SCHEMA_JSON).expect("product schema is JSON");
        let defs = schema["$defs"]
            .as_object()
            .expect("product schema defines $defs")
            .clone();
        let methods = document["methods"]
            .as_array()
            .expect("surface catalog lists methods")
            .iter()
            .map(|method| Method {
                name: method["name"].as_str().expect("method name").to_owned(),
                params: method["params"].as_str().map(str::to_owned),
                grant: method["grant"].as_str().map(str::to_owned),
                read_only: method["readOnly"].as_bool().expect("readOnly"),
                destructive: method["destructive"].as_bool().expect("destructive"),
            })
            .collect();
        let excluded = document["excluded"]
            .as_array()
            .expect("surface catalog lists exclusions")
            .iter()
            .map(|entry| {
                (
                    entry["method"]
                        .as_str()
                        .expect("excluded method")
                        .to_owned(),
                    entry["reason"]
                        .as_str()
                        .expect("exclusion reason")
                        .to_owned(),
                )
            })
            .collect();
        Catalog {
            methods,
            excluded,
            defs,
        }
    })
}

/// A method the catalog exposes, with params the schema accepted.
pub struct Prepared {
    pub method: String,
    pub params: Value,
}

/// Why a call never reached the gateway, or reached it and was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// Not a manifest method.
    Unknown,
    /// A manifest method this surface does not expose, with its reason.
    Excluded { reason: String },
    /// Params are not the method's schema.
    Invalid,
}

/// Look the method up and check `params` against its schema.
pub fn prepare(method: &str, params: &Value) -> Result<Prepared, PrepareError> {
    let catalog = catalog();
    if let Some(reason) = catalog.excluded.get(method) {
        return Err(PrepareError::Excluded {
            reason: reason.clone(),
        });
    }
    let Some(found) = catalog.methods.iter().find(|entry| entry.name == method) else {
        return Err(PrepareError::Unknown);
    };
    if !params_match(found, params) {
        return Err(PrepareError::Invalid);
    }
    Ok(Prepared {
        method: found.name.clone(),
        params: params.clone(),
    })
}

/// One MCP tool per exposed method, in catalog order.
pub fn tools() -> Vec<Value> {
    catalog()
        .methods
        .iter()
        .map(|method| {
            json!({
                "name": method.name,
                "description": description(method),
                "inputSchema": input_schema(method),
                "annotations": {
                    "readOnlyHint": method.read_only,
                    "destructiveHint": method.destructive,
                },
            })
        })
        .collect()
}

fn description(method: &Method) -> String {
    let grant = match &method.grant {
        Some(grant) => format!("Cedar asks for {grant}."),
        None => "The session admits it before Cedar is asked.".to_owned(),
    };
    let written = method.params.as_ref().and_then(|name| {
        catalog().defs.get(name).and_then(|schema| {
            schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    });
    match written {
        Some(text) => format!("{text} {grant}"),
        None => format!("Gateway method {}. {grant}", method.name),
    }
}

fn input_schema(method: &Method) -> Value {
    let Some(params) = &method.params else {
        return json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        });
    };
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("nessa://protocol/product/surface/{params}"),
        "$defs": reachable(params),
        "$ref": format!("#/$defs/{params}"),
    })
}

fn reachable(root: &str) -> Map<String, Value> {
    let defs = &catalog().defs;
    let mut needed = BTreeSet::new();
    let mut pending = vec![root.to_owned()];
    while let Some(name) = pending.pop() {
        if !needed.insert(name.clone()) {
            continue;
        }
        let Some(schema) = defs.get(&name) else {
            continue;
        };
        let mut refs = BTreeSet::new();
        collect_refs(schema, &mut refs);
        pending.extend(refs);
    }
    needed
        .into_iter()
        .filter_map(|name| defs.get(&name).map(|schema| (name, schema.clone())))
        .collect()
}

fn collect_refs(node: &Value, into: &mut BTreeSet<String>) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect_refs(item, into);
            }
        }
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
            {
                into.insert(name.to_owned());
            }
            for value in map.values() {
                collect_refs(value, into);
            }
        }
        _ => {}
    }
}

fn params_match(method: &Method, params: &Value) -> bool {
    let schema = input_schema(method);
    let mut validators = validators().lock().expect("validator cache");
    if !validators.contains_key(&method.name) {
        let compiled = match compile(&schema) {
            Ok(compiled) => compiled,
            Err(error) => {
                tracing::error!(method = %method.name, %error, "method schema did not compile");
                return false;
            }
        };
        validators.insert(method.name.clone(), compiled);
    }
    validators
        .get(&method.name)
        .expect("validator just stored")
        .is_valid(params)
}

fn validators() -> &'static Mutex<HashMap<String, jsonschema::Validator>> {
    static VALIDATORS: OnceLock<Mutex<HashMap<String, jsonschema::Validator>>> = OnceLock::new();
    VALIDATORS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn compile(schema: &Value) -> Result<jsonschema::Validator, jsonschema::ValidationError<'static>> {
    jsonschema::options()
        .offline()
        .with_keyword("x-utf8MaxBytes", utf8_max_bytes)
        .with_keyword("x-maxTotalBytes", max_total_bytes)
        .build(schema)
}

struct Utf8MaxBytes(usize);

impl<'i> jsonschema::Keyword<'i> for Utf8MaxBytes {
    fn validate(&self, instance: &'i Value) -> Result<(), jsonschema::ValidationError<'i>> {
        if self.is_valid(instance) {
            Ok(())
        } else {
            Err(jsonschema::ValidationError::custom(
                "exceeds the UTF-8 byte bound",
            ))
        }
    }

    fn is_valid(&self, instance: &'i Value) -> bool {
        instance.as_str().is_none_or(|text| text.len() <= self.0)
    }
}

struct MaxTotalBytes(u64);

impl<'i> jsonschema::Keyword<'i> for MaxTotalBytes {
    fn validate(&self, instance: &'i Value) -> Result<(), jsonschema::ValidationError<'i>> {
        if self.is_valid(instance) {
            Ok(())
        } else {
            Err(jsonschema::ValidationError::custom(
                "exceeds the total byte bound",
            ))
        }
    }

    fn is_valid(&self, instance: &'i Value) -> bool {
        let Some(items) = instance.as_array() else {
            return true;
        };
        let total = items.iter().fold(0_u64, |total, item| {
            total.saturating_add(item.get("size").and_then(Value::as_u64).unwrap_or(0))
        });
        total <= self.0
    }
}

fn utf8_max_bytes<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _location: jsonschema::paths::Location,
) -> Result<Box<dyn for<'i> jsonschema::Keyword<'i>>, jsonschema::ValidationError<'a>> {
    let limit = value.as_u64().ok_or_else(|| {
        jsonschema::ValidationError::custom("x-utf8MaxBytes must be a byte count")
    })?;
    let limit = usize::try_from(limit).map_err(|_| {
        jsonschema::ValidationError::custom("x-utf8MaxBytes does not fit this process")
    })?;
    Ok(Box::new(Utf8MaxBytes(limit)))
}

fn max_total_bytes<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _location: jsonschema::paths::Location,
) -> Result<Box<dyn for<'i> jsonschema::Keyword<'i>>, jsonschema::ValidationError<'a>> {
    let limit = value.as_u64().ok_or_else(|| {
        jsonschema::ValidationError::custom("x-maxTotalBytes must be a byte count")
    })?;
    Ok(Box::new(MaxTotalBytes(limit)))
}
