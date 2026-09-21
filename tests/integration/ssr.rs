//! Tests the structural search and replace, whose patterns are resolved rather than matched.
//!
//! This is the one rust-analyzer extension the fork's handoff left unprobed, and the two ways it
//! could have been absent are both silent: an unknown method and a method that answers nothing
//! look the same from here if the only thing asserted is that no error came back. So the query
//! below is one with a known answer in `test-project`.

use anyhow::Result;
use serde_json::{json, Value};
use test_support::IpcClient;

#[tokio::test]
async fn a_pattern_is_rewritten_across_the_workspace() -> Result<()> {
    let mut client = IpcClient::get_or_create("test-project").await?;
    let fixture = client.workspace_path().join("src/blast_fixture.rs");

    let answer = call(
        &mut client,
        "rust_analyzer_ssr",
        json!({
            "file_path": fixture.to_str().unwrap(),
            "query": "shared_helper($a) ==>> shared_helper($a + 0)",
        }),
    )
    .await?;

    // Asserting that an edit came back is not enough: an edit with nothing in it is what a
    // method that answers politely and does nothing returns, and it would pass. What the rewrite
    // says is the check that can fail.
    let rewritten: Vec<&str> = answer["documentChanges"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|change| change["edits"].as_array())
        .flatten()
        .filter_map(|edit| edit["newText"].as_str())
        .collect();

    assert!(
        !rewritten.is_empty(),
        "`shared_helper` is called in this project, so the rewrite is not empty: {answer}"
    );
    assert!(
        rewritten.iter().any(|text| text.contains("+ 0")),
        "the replacement half of the query is what the edits are expected to say: {rewritten:?}"
    );

    Ok(())
}

/// The cheap way to find out a pattern is wrong, which is worth having only if it can tell a
/// wrong pattern from a right one.
#[tokio::test]
async fn a_query_that_is_not_a_pattern_is_refused() -> Result<()> {
    let mut client = IpcClient::get_or_create("test-project").await?;
    let fixture = client.workspace_path().join("src/blast_fixture.rs");

    let complaint = client
        .call_tool(
            "rust_analyzer_ssr",
            json!({
                "file_path": fixture.to_str().unwrap(),
                "query": "this is not a pattern at all",
                "parse_only": true,
            }),
        )
        .await
        .expect_err("a query with no `==>>` in it is not a rewrite");

    assert!(
        complaint.to_string().contains("==>>"),
        "the error is expected to say what a query looks like: {complaint}"
    );

    Ok(())
}

/// The same check the other way round: a well-formed pattern must not be refused, or the one
/// above would pass for a tool that refuses everything.
#[tokio::test]
async fn a_well_formed_pattern_passes_the_same_check() -> Result<()> {
    let mut client = IpcClient::get_or_create("test-project").await?;
    let fixture = client.workspace_path().join("src/blast_fixture.rs");

    call(
        &mut client,
        "rust_analyzer_ssr",
        json!({
            "file_path": fixture.to_str().unwrap(),
            "query": "shared_helper($a) ==>> shared_helper($a + 0)",
            "parse_only": true,
        }),
    )
    .await?;

    Ok(())
}

/// The parsed answer of a tool that replies with JSON in a text content item.
async fn call(client: &mut IpcClient, tool: &str, arguments: Value) -> Result<Value> {
    let response = client.call_tool(tool, arguments).await?;
    let text = response["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a text content item, got: {response}"));

    Ok(serde_json::from_str(text)?)
}
