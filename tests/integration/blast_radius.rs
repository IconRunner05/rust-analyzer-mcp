//! Tests the one answer a blast radius gives that a reference list does not: which of the hits
//! are tests.
//!
//! `src/blast_fixture.rs` is built for this. Its symbol is called once from production code and
//! twice from a `#[cfg(test)] mod tests` in the same file, so a split made by filename reports
//! three production callers and no tests -- the numbers below are wrong in both directions under
//! such a rule, which is what makes them worth asserting.

use anyhow::Result;
use serde_json::{json, Value};
use test_support::IpcClient;

#[tokio::test]
async fn callers_in_one_file_are_split_into_tests_and_production() -> Result<()> {
    let mut client = IpcClient::get_or_create("test-project").await?;
    let fixture = client.workspace_path().join("src/blast_fixture.rs");

    // `pub fn shared_helper`, on the name itself.
    let answer = call(
        &mut client,
        "rust_analyzer_blast_radius",
        json!({ "file_path": fixture.to_str().unwrap(), "line": 9, "character": 7 }),
    )
    .await?;

    assert_eq!(
        answer["complete"],
        json!(true),
        "every referencing file could be classified: {answer}"
    );
    assert!(
        answer["target"]["signature"]
            .as_str()
            .is_some_and(|signature| signature.contains("shared_helper")),
        "the hover proving the position is on the symbol is reported: {answer}"
    );

    assert_eq!(
        answer["production"]["count"],
        json!(1),
        "`production_caller` is the only caller outside the test module: {answer}"
    );
    assert_eq!(
        answer["production"]["references"][0]["item"],
        json!("production_caller"),
        "a production hit is named by the item it is inside: {answer}"
    );

    assert_eq!(
        answer["tests"]["count"],
        json!(2),
        "both hits inside `mod tests` are tests, though the file is not named like one: {answer}"
    );

    let named: Vec<&str> = answer["tests"]["references"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|hit| hit["test"].as_str())
        .collect();
    assert!(
        named
            .iter()
            .any(|label| label.contains("the_helper_adds_one")),
        "the hit inside a `#[test]` is named by that test: {named:?}"
    );
    assert!(
        named.iter().any(|label| label.starts_with("test-mod")),
        "the hit inside a plain helper in `mod tests` has only the module's runnable to be \
         named by, and that is what keeps it out of the production count: {named:?}"
    );

    // The declaration is neither a test caller nor a production one, and is lifted out of both.
    assert_eq!(
        answer["declaration"]["line"],
        json!(9),
        "the declaring line is reported apart from the callers: {answer}"
    );

    Ok(())
}

/// The check the tool makes before it searches, and the reason an empty answer from it means
/// something. A position that is not on a symbol answers every later question with an empty list,
/// which reads exactly like a symbol nothing uses.
#[tokio::test]
async fn a_position_that_is_not_on_a_symbol_is_refused_rather_than_answered_emptily() -> Result<()>
{
    let mut client = IpcClient::get_or_create("test-project").await?;
    let fixture = client.workspace_path().join("src/blast_fixture.rs");

    // Inside the file's module doc comment, where nothing is declared.
    let complaint = client
        .call_tool(
            "rust_analyzer_blast_radius",
            json!({ "file_path": fixture.to_str().unwrap(), "line": 1, "character": 4 }),
        )
        .await
        .expect_err("a doc comment is not a symbol");

    assert!(
        complaint.to_string().contains("no hover there"),
        "the error is expected to say the position is not on a symbol: {complaint}"
    );

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
