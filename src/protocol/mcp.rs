use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub struct MCPRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MCPResponse {
    Success {
        jsonrpc: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<Value>,
        result: Value,
    },
    Error {
        jsonrpc: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<Value>,
        error: MCPError,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MCPError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

/// What a client shows and assumes about a tool, as distinct from what the tool takes.
///
/// A client with no annotations to go on shows the wire name -- `rust_analyzer_workspace_symbols`
/// in a list of other `rust_analyzer_` prefixes -- and has to assume every call might change
/// something, so every call is worth asking the user about. Both of those are answered here and
/// neither can be answered by the schema.
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// The name a person reads, instead of the wire name.
    pub title: String,
    /// Whether the tool leaves everything as it found it.
    ///
    /// The three tools that work out an edit -- `format`, `rename`, `ssr` -- are read-only under
    /// this, and that is not a stretch: they return the edit and write nothing, so the decision
    /// to change a file is still the caller's and has not been taken by asking.
    #[serde(rename = "readOnlyHint")]
    pub read_only_hint: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ContentItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}
