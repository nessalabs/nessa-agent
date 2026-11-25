//! Wire codecs and shared JSON-RPC method dispatch for Skills over MCP
//! (SEP-2640, `io.modelcontextprotocol/skills`).
//!
//! Both client-side decoding (`McpSession`) and server-side dispatch
//! (`BuiltInServer` and `nessa mcp`) share this module so wire shapes and
//! verification rules have one owner.
use crate::domain::mcp_skills::{
    DirectoryChild, FrontmatterValue, McpSkillError, SkillCatalog, SkillDigest, SkillEntry,
    SkillFileUri, SkillFrontmatter, SkillResourceEntry, SkillResources, SkillUri, SkillsCapability,
    EXTENSION,
};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use super::McpError;

/// Maximum pages a client follows for `skills/list` or `resources/directory/read`.
pub(crate) const MAX_SKILL_PAGES: usize = 32;
/// Maximum skills returned by one bounded `list_skills` walk.
pub(crate) const MAX_SKILLS: usize = 1024;
/// Default page size for server-side `skills/list` responses.
const DEFAULT_PAGE_SIZE: usize = 64;
/// Default list-cache TTL (5 minutes) advertised on static skill responses.
const STATIC_CACHE_TTL_MS: u64 = 300_000;

/// Verified bytes of a skill file retrieved through `resources/read` and
/// checked against its retained [`SkillEntry`] manifest (and frontmatter for
/// `SKILL.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedSkillFile {
    /// Resource URI that was read and verified.
    pub uri: SkillFileUri,
    /// MIME type reported by the server.
    pub mime_type: String,
    /// Verified raw bytes.
    pub bytes: Vec<u8>,
}

impl VerifiedSkillFile {
    /// Borrow the verified content as UTF-8 text, if valid UTF-8.
    pub fn as_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

/// Read the server's `io.modelcontextprotocol/skills` capability from its
/// `initialize` result.
///
/// Per SEP-2640, a server supports Skills only when it declares **both**
/// `capabilities.resources` and `capabilities.extensions["io.modelcontextprotocol/skills"]`.
pub(crate) fn server_skills_capability(initialized: &Value) -> Option<SkillsCapability> {
    let caps = initialized.get("capabilities")?.as_object()?;
    if !caps.get("resources").is_some_and(Value::is_object) {
        return None;
    }
    let ext = caps
        .get("extensions")?
        .as_object()?
        .get(EXTENSION)?
        .as_object()?;
    let directory_read = ext
        .get("directoryRead")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(SkillsCapability { directory_read })
}

/// Add SEP-2640 `resources` and `extensions["io.modelcontextprotocol/skills"]`
/// entries to a server's `initialize` capabilities object.
pub fn insert_server_skills_capabilities(capabilities: &mut Map<String, Value>) {
    capabilities
        .entry("resources".to_owned())
        .or_insert_with(|| json!({}));
    let extensions = capabilities
        .entry("extensions".to_owned())
        .or_insert_with(|| json!({}));
    if let Some(ext_map) = extensions.as_object_mut() {
        ext_map.insert(EXTENSION.to_owned(), json!({ "directoryRead": true }));
    }
}

/// Handle a SEP-2640 or base `resources/*` JSON-RPC method against a static
/// [`SkillCatalog`].
///
/// Returns `None` for methods outside this surface (`initialize`, `tools/*`,
/// etc.). Returns `Some(Ok(result))` or `Some(Err((json_rpc_code, message)))`
/// for `skills/list`, `skills/get`, `resources/list`, `resources/read`, and
/// `resources/directory/read`.
pub fn handle_skill_method(
    catalog: &SkillCatalog,
    method: &str,
    params: &Value,
) -> Option<Result<Value, (i64, String)>> {
    match method {
        "skills/list" => Some(handle_skills_list(catalog, params)),
        "skills/get" => Some(handle_skills_get(catalog, params)),
        "resources/list" => Some(Ok(handle_resources_list(catalog))),
        "resources/read" => Some(handle_resources_read(catalog, params)),
        "resources/directory/read" => Some(handle_directory_read(catalog, params)),
        _ => None,
    }
}

fn handle_skills_list(catalog: &SkillCatalog, params: &Value) -> Result<Value, (i64, String)> {
    let entries = catalog.entries();
    let offset = match params.get("cursor") {
        None | Some(Value::Null) => 0usize,
        Some(Value::String(cursor)) => cursor
            .parse::<usize>()
            .ok()
            .filter(|idx| *idx <= entries.len())
            .ok_or_else(|| (-32602, "invalid skills/list cursor".to_owned()))?,
        Some(_) => return Err((-32602, "skills/list cursor must be a string".to_owned())),
    };
    let end = (offset + DEFAULT_PAGE_SIZE).min(entries.len());
    let skills: Vec<Value> = entries[offset..end]
        .iter()
        .map(encode_skill_entry)
        .collect();
    let mut result = Map::new();
    result.insert("resultType".to_owned(), json!("complete"));
    result.insert("skills".to_owned(), Value::Array(skills));
    result.insert("ttlMs".to_owned(), json!(STATIC_CACHE_TTL_MS));
    result.insert("cacheScope".to_owned(), json!("public"));
    if end < entries.len() {
        result.insert("nextCursor".to_owned(), json!(end.to_string()));
    }
    Ok(Value::Object(result))
}

fn handle_skills_get(catalog: &SkillCatalog, params: &Value) -> Result<Value, (i64, String)> {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return Err((-32602, "skills/get requires a string uri".to_owned()));
    };
    let Some(entry) = catalog.get(uri) else {
        return Err((-32602, format!("unknown skill uri: {uri}")));
    };
    Ok(json!({
        "resultType": "complete",
        "skill": encode_skill_entry(entry),
        "ttlMs": STATIC_CACHE_TTL_MS,
        "cacheScope": "public",
    }))
}

fn handle_resources_list(catalog: &SkillCatalog) -> Value {
    let resources: Vec<Value> = catalog
        .list_resources()
        .into_iter()
        .map(|(uri, name, description, mime_type)| {
            let mut obj = Map::new();
            obj.insert("uri".to_owned(), json!(uri));
            obj.insert("name".to_owned(), json!(name));
            if let Some(desc) = description {
                obj.insert("description".to_owned(), json!(desc));
            }
            obj.insert("mimeType".to_owned(), json!(mime_type));
            Value::Object(obj)
        })
        .collect();
    json!({
        "resultType": "complete",
        "resources": resources,
        "ttlMs": STATIC_CACHE_TTL_MS,
        "cacheScope": "public",
    })
}

fn handle_resources_read(catalog: &SkillCatalog, params: &Value) -> Result<Value, (i64, String)> {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return Err((-32602, "resources/read requires a string uri".to_owned()));
    };
    let Some((res_uri, mime_type, bytes)) = catalog.read_resource(uri) else {
        return Err((-32602, format!("unknown resource uri: {uri}")));
    };
    let item = match std::str::from_utf8(bytes) {
        Ok(text) => json!({
            "uri": res_uri,
            "mimeType": mime_type,
            "text": text,
        }),
        Err(_) => json!({
            "uri": res_uri,
            "mimeType": mime_type,
            "blob": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    };
    Ok(json!({
        "resultType": "complete",
        "contents": [item],
        "ttlMs": STATIC_CACHE_TTL_MS,
        "cacheScope": "public",
    }))
}

fn handle_directory_read(catalog: &SkillCatalog, params: &Value) -> Result<Value, (i64, String)> {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return Err((
            -32602,
            "resources/directory/read requires a string uri".to_owned(),
        ));
    };
    let children = catalog
        .read_directory(uri)
        .map_err(|err| (-32602, err.to_string()))?;
    let resources: Vec<Value> = children
        .iter()
        .map(|child| {
            json!({
                "uri": child.uri,
                "name": child.name,
                "mimeType": child.mime_type,
            })
        })
        .collect();
    Ok(json!({
        "resultType": "complete",
        "resources": resources,
        "ttlMs": STATIC_CACHE_TTL_MS,
        "cacheScope": "public",
    }))
}

/// Encode a [`SkillEntry`] into its SEP-2640 JSON shape.
pub(crate) fn encode_skill_entry(entry: &SkillEntry) -> Value {
    let resources = match entry.resources() {
        SkillResources::Dynamic => json!("dynamic"),
        SkillResources::Manifest(files) => Value::Array(
            files
                .iter()
                .map(|file| {
                    json!({
                        "uri": file.uri().as_str(),
                        "digest": file.digest().as_str(),
                        "size": file.size(),
                    })
                })
                .collect(),
        ),
    };
    json!({
        "uri": entry.uri().as_str(),
        "frontmatter": frontmatter_to_json(entry.frontmatter().fields()),
        "resources": resources,
    })
}

/// Decode one page of a `skills/list` result into validated [`SkillEntry`] values.
pub(crate) fn decode_skills_list_page(
    result: &Value,
) -> Result<(Vec<SkillEntry>, Option<String>), McpError> {
    let skills = result
        .get("skills")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("skills/list without a skills array"))?;
    let mut out = Vec::with_capacity(skills.len());
    for raw in skills {
        out.push(decode_skill_entry(raw)?);
    }
    let next = match result.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
        Some(Value::String(_)) => None,
        Some(_) => return Err(malformed("skills/list nextCursor is not a string")),
    };
    Ok((out, next))
}

/// Decode a `skills/get` result into a validated [`SkillEntry`].
pub(crate) fn decode_skills_get(result: &Value) -> Result<SkillEntry, McpError> {
    let raw = result
        .get("skill")
        .ok_or_else(|| malformed("skills/get without a skill object"))?;
    decode_skill_entry(raw)
}

/// Decode a `resources/read` result for a skill file into `(mime_type, raw_bytes)`.
pub(crate) fn decode_skill_resource(
    requested: &SkillFileUri,
    result: &Value,
) -> Result<(String, Vec<u8>), McpError> {
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("resources/read without a contents array"))?;
    let [item] = contents.as_slice() else {
        return Err(malformed("resources/read must return one content item"));
    };
    let uri = item
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("resources/read content without uri"))?;
    if uri != requested.as_str() {
        return Err(malformed("resources/read returned a different uri"));
    }
    let mime_type = item
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream")
        .to_owned();
    let bytes = match (
        item.get("text").and_then(Value::as_str),
        item.get("blob").and_then(Value::as_str),
    ) {
        (Some(text), None) => text.as_bytes().to_vec(),
        (None, Some(blob)) => base64::engine::general_purpose::STANDARD
            .decode(blob)
            .map_err(|_| malformed("resources/read blob is not valid base64"))?,
        _ => {
            return Err(malformed(
                "resources/read content must carry exactly one of text or blob",
            ))
        }
    };
    Ok((mime_type, bytes))
}

/// Decode one page of a `resources/directory/read` result.
pub(crate) fn decode_directory_read_page(
    result: &Value,
) -> Result<(Vec<DirectoryChild>, Option<String>), McpError> {
    let list = result
        .get("resources")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("resources/directory/read without a resources array"))?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let uri = item
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("directory child without uri"))?;
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("directory child without name"))?;
        let mime_type = item
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream");
        out.push(DirectoryChild {
            uri: uri.to_owned(),
            name: name.to_owned(),
            mime_type: mime_type.to_owned(),
        });
    }
    let next = match result.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor.clone()),
        Some(Value::String(_)) => None,
        Some(_) => {
            return Err(malformed(
                "resources/directory/read nextCursor is not a string",
            ))
        }
    };
    Ok((out, next))
}

fn decode_skill_entry(raw: &Value) -> Result<SkillEntry, McpError> {
    let obj = raw
        .as_object()
        .ok_or_else(|| malformed("skill entry is not an object"))?;
    let uri_str = obj
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("skill entry without uri"))?;
    let uri = SkillUri::new(uri_str).map_err(skill_err)?;

    let fm_obj = obj
        .get("frontmatter")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed("skill entry without frontmatter object"))?;
    let fields = json_map_to_frontmatter(fm_obj)?;
    let frontmatter = SkillFrontmatter::new(fields).map_err(skill_err)?;

    let resources = match obj.get("resources") {
        Some(Value::String(marker)) if marker == "dynamic" => SkillResources::Dynamic,
        Some(Value::Array(items)) => {
            let mut entries = Vec::with_capacity(items.len());
            for item in items {
                let res_obj = item
                    .as_object()
                    .ok_or_else(|| malformed("skill resource entry is not an object"))?;
                let res_uri = res_obj
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("skill resource entry without uri"))?;
                let digest = res_obj
                    .get("digest")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("skill resource entry without digest"))?;
                let size = res_obj
                    .get("size")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| malformed("skill resource entry without integer size"))?;
                entries.push(SkillResourceEntry::new(
                    SkillFileUri::new(res_uri).map_err(skill_err)?,
                    SkillDigest::parse(digest).map_err(skill_err)?,
                    size,
                ));
            }
            SkillResources::Manifest(entries)
        }
        _ => {
            return Err(malformed(
                "skill entry resources must be an array or \"dynamic\"",
            ))
        }
    };

    SkillEntry::new(uri, frontmatter, resources).map_err(skill_err)
}

fn frontmatter_to_json(fields: &BTreeMap<String, FrontmatterValue>) -> Value {
    let mut map = Map::new();
    for (k, v) in fields {
        map.insert(k.clone(), fm_value_to_json(v));
    }
    Value::Object(map)
}

fn fm_value_to_json(value: &FrontmatterValue) -> Value {
    match value {
        FrontmatterValue::Null => Value::Null,
        FrontmatterValue::Bool(b) => Value::Bool(*b),
        FrontmatterValue::Integer(n) => json!(*n),
        FrontmatterValue::String(s) => Value::String(s.clone()),
        FrontmatterValue::Sequence(items) => {
            Value::Array(items.iter().map(fm_value_to_json).collect())
        }
        FrontmatterValue::Mapping(map) => frontmatter_to_json(map),
    }
}

fn json_map_to_frontmatter(
    map: &Map<String, Value>,
) -> Result<BTreeMap<String, FrontmatterValue>, McpError> {
    let mut out = BTreeMap::new();
    for (k, v) in map {
        out.insert(k.clone(), json_to_fm_value(v)?);
    }
    Ok(out)
}

fn json_to_fm_value(value: &Value) -> Result<FrontmatterValue, McpError> {
    match value {
        Value::Null => Ok(FrontmatterValue::Null),
        Value::Bool(b) => Ok(FrontmatterValue::Bool(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(FrontmatterValue::Integer(i))
            } else {
                Ok(FrontmatterValue::String(n.to_string()))
            }
        }
        Value::String(s) => Ok(FrontmatterValue::String(s.clone())),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_fm_value(item)?);
            }
            Ok(FrontmatterValue::Sequence(out))
        }
        Value::Object(map) => Ok(FrontmatterValue::Mapping(json_map_to_frontmatter(map)?)),
    }
}

fn malformed(message: &str) -> McpError {
    McpError::Malformed(message.to_owned())
}

fn skill_err(err: McpSkillError) -> McpError {
    McpError::Malformed(err.to_string())
}
