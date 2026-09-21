//! What actually breaks if the symbol at a position changes.
//!
//! The question a reference list is usually standing in for is "what breaks if I change this",
//! and a flat list of hits answers it badly in two ways. It does not say which enclosing thing
//! each hit is in, so the reader opens N files to find out. And it does not say which hits are
//! tests, which is the difference between a change that needs the callers updated and a change
//! that needs the callers *and* the suite rewritten.
//!
//! Splitting tests off by filename is how this is usually done, and in Rust it is wrong. A crate's
//! unit tests live in a `#[cfg(test)] mod tests` *inside the file they test*, so a filename rule
//! files every one of them under production and reports a test-caller count of zero -- which reads
//! exactly like a symbol no test covers. rust-analyzer already knows better: it offers to run each
//! test, and each offer carries the range of the thing it would run. A hit inside one of those
//! ranges is a test hit, whatever the file is called, and the offer's label names the test for
//! free. One kind of offer does not carry that range and cannot be used -- a doctest's range is
//! the item its doc comment is attached to, body included; `is_test_label` is where that is
//! handled and why.
//!
//! What this module cannot see is a *copy*. Reference edges are resolved from one definition, so a
//! byte-identical function pasted into another crate has no edge to this one and appears in no
//! answer here. A repo-wide text sweep is the other half of that question and this is not it.

use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A span of one file that rust-analyzer offers to run: a `#[test]` function, a `#[cfg(test)]`
/// module, a doctest, a benchmark.
///
/// `label` is rust-analyzer's own wording (`test uri::tests::unix_paths_round_trip`,
/// `test-mod uri::tests`), kept verbatim because it says both which test and what kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSpan {
    pub label: String,
    pub start_line: u64,
    pub end_line: u64,
}

impl TestSpan {
    fn contains(&self, line: u64) -> bool {
        self.start_line <= line && line <= self.end_line
    }

    fn lines(&self) -> u64 {
        self.end_line.saturating_sub(self.start_line)
    }
}

/// The runnables of one file, reduced to the spans that are tests.
///
/// Two things are dropped. Runnables with no `location` -- `cargo check -p <crate>` and
/// `cargo test -p <crate>`, which stand for the whole package rather than a place in the file --
/// would otherwise be spans with no range. And runnables whose range is not a range of test code:
/// a `run` runnable for `fn main` is an offer to run the program, not a test, and a `doctest`'s
/// range is the documented item rather than the doctest. `is_test_label` decides both.
pub fn test_spans(runnables: &Value) -> Vec<TestSpan> {
    let Some(items) = runnables.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let label = item.get("label").and_then(Value::as_str)?;
            if !is_test_label(label) {
                return None;
            }
            let start_line = item
                .pointer("/location/targetRange/start/line")
                .and_then(Value::as_u64)?;
            let end_line = item
                .pointer("/location/targetRange/end/line")
                .and_then(Value::as_u64)?;
            Some(TestSpan {
                label: label.to_string(),
                start_line,
                end_line,
            })
        })
        .collect()
}

/// Whether a runnable's label describes something that is not production code.
///
/// rust-analyzer's labels are a kind followed by a path, and the kind is the whole of what is
/// needed here: `test`, `test-mod` and `bench` are all code that ships with the crate but does not
/// run in it. The distinction being drawn is not "is it a `#[test]`" but "would a caller here
/// constrain the production API", and a benchmark constrains it the same way a test does -- so it
/// goes on the same side, under its own label, which says which it was.
///
/// `doctest` is deliberately NOT among them, and it is the one kind whose range cannot be used.
/// The others are offers to run a span of code and their range is that code; a doctest's range is
/// **the item the doc comment is attached to** -- its signature and its whole body, none of which
/// is the doctest. Measured on a two-caller fixture: `doctest documented_caller` came back
/// spanning lines 0 to 7, which is the doc comment, the `pub fn` line and the body under it.
/// Treating that as a test span files every reference inside a documented function under `tests`,
/// so a heavily-used helper answers `production: 0` on a crate whose standard is to document its
/// public surface -- and the better documented the code, the more of it disappears.
///
/// Nothing is lost by dropping it, because no reference edge ever lands in a doctest. Doctest code
/// is a string in a comment, compiled as its own crate, and rust-analyzer does not resolve it:
/// measured on the same fixture, a `pub fn` called in its own doctest has a reference count of 1,
/// the declaration alone. So a doctest span can never capture a hit that belongs to it. It can
/// only take one that belongs to the item it is written on.
fn is_test_label(label: &str) -> bool {
    let kind = label.split_whitespace().next().unwrap_or_default();
    matches!(kind, "test" | "test-mod" | "bench")
}

/// The innermost test span covering a line, or `None` if the line is not in a test.
///
/// A `#[test]` function inside a `#[cfg(test)] mod tests` is covered by two spans, the module's
/// and its own. The narrower one is the useful answer -- it names the single test to run rather
/// than the whole module -- so ties go to the shorter span.
fn innermost(spans: &[TestSpan], line: u64) -> Option<&TestSpan> {
    spans
        .iter()
        .filter(|span| span.contains(line))
        .min_by_key(|span| span.lines())
}

/// The path of named items enclosing a line, from a hierarchical `DocumentSymbol` tree, joined
/// the way Rust writes it.
///
/// A reference is reported as a position, and a position on its own does not say what it is a
/// reference *from*. The enclosing item does: `handle_references` is a caller, `Calculator::add`
/// is a method that mentions the symbol. This walks to the deepest item whose range covers the
/// line, so a method inside an impl inside a module comes back fully qualified.
///
/// Doc comments are inside `range` and outside `selectionRange`, which is the opposite of what
/// the position-finding tools want but exactly right here: a mention in the doc comment of a
/// function is still a mention from that function.
pub fn enclosing_item(symbols: &Value, line: u64) -> Option<String> {
    let mut path = Vec::new();
    let mut level = symbols.as_array()?;

    // Descending stops at the deepest item that covers the line, which is not always a leaf: a
    // line between two methods is inside the impl and inside neither of them, and the impl is the
    // answer. Running out of covering children ends the walk with what has been found rather than
    // discarding it.
    while let Some(deepest) = level
        .iter()
        .filter(|symbol| covers(symbol, line))
        .min_by_key(|symbol| span_lines(symbol).unwrap_or(u64::MAX))
    {
        if let Some(name) = deepest.get("name").and_then(Value::as_str) {
            path.push(name.to_string());
        }

        match deepest.get("children").and_then(Value::as_array) {
            Some(children) if !children.is_empty() => level = children,
            _ => break,
        }
    }

    (!path.is_empty()).then(|| path.join("::"))
}

/// The range of a symbol, from either shape rust-analyzer may send: `range` on a hierarchical
/// `DocumentSymbol`, `location.range` on a flat `SymbolInformation`.
fn symbol_range(symbol: &Value) -> Option<(u64, u64)> {
    let start = symbol
        .pointer("/range/start/line")
        .or_else(|| symbol.pointer("/location/range/start/line"))
        .and_then(Value::as_u64)?;
    let end = symbol
        .pointer("/range/end/line")
        .or_else(|| symbol.pointer("/location/range/end/line"))
        .and_then(Value::as_u64)?;
    Some((start, end))
}

fn covers(symbol: &Value, line: u64) -> bool {
    symbol_range(symbol).is_some_and(|(start, end)| start <= line && line <= end)
}

fn span_lines(symbol: &Value) -> Option<u64> {
    symbol_range(symbol).map(|(start, end)| end.saturating_sub(start))
}

/// The declaration rust-analyzer shows on hover, without the prose under it.
///
/// A hover is one or more fenced `rust` blocks followed by however much rustdoc the item carries,
/// which for a well-documented type is most of a screen. The blocks are what say the position
/// landed on the symbol that was meant; the prose is already readable where it is written.
///
/// There is usually more than one block and the first is not the signature -- rust-analyzer leads
/// with the module path (`test_project::blast_fixture`) and gives the signature after it -- so
/// every block is kept, in order. Together they are two short lines.
///
/// `None` means the hover said nothing, which is the one answer this whole tool refuses to
/// search past: a position that is not on a symbol answers every later question with an empty
/// list that reads exactly like a symbol nothing uses.
pub fn hover_signature(hover: &Value) -> Option<String> {
    let text = hover
        .pointer("/contents/value")
        .and_then(Value::as_str)
        .or_else(|| hover.get("contents").and_then(Value::as_str))?;

    if text.trim().is_empty() {
        return None;
    }

    let mut fenced = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("```") {
            inside = line.starts_with("```rust");
            continue;
        }
        if inside {
            fenced.push(line);
        }
    }

    let signature = if fenced.is_empty() {
        text.trim().to_string()
    } else {
        fenced.join("\n").trim().to_string()
    };

    (!signature.is_empty()).then_some(signature)
}

/// Everything known about one file that references are being classified against.
#[derive(Debug, Default)]
pub struct FileContext {
    /// The test spans of the file, or `None` if rust-analyzer would not say. The difference
    /// matters: an empty list means the file has no tests in it, and `None` means nobody knows,
    /// which is not a thing to report as production.
    pub test_spans: Option<Vec<TestSpan>>,
    /// The file's symbol tree, used to name what each hit is inside.
    pub symbols: Value,
}

/// The finished answer.
///
/// `complete` is the field to read before the counts. It is false when any file could not be
/// classified, and a zero under it means "nothing was worked out", not "nothing is there" -- the
/// distinction that makes the difference between a safe change and a silent break.
pub fn assemble(
    target: Value,
    annotated: &Value,
    context: &BTreeMap<String, FileContext>,
    implementations: &Value,
) -> Value {
    let hits = annotated
        .get("locations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut declaration = Value::Null;
    let mut production = Vec::new();
    let mut tests = Vec::new();
    let mut unclassified = Vec::new();
    let mut files_touched: Vec<String> = Vec::new();

    for hit in hits {
        let file = hit.get("file").and_then(Value::as_str).unwrap_or_default();
        if !files_touched.iter().any(|seen| seen == file) {
            files_touched.push(file.to_string());
        }

        // The declaration is among the references and is not a use of the symbol. Counting it as
        // a caller is the off-by-one the reference tool already marks; here it is lifted out
        // entirely, so both counts are counts of code that would have to change.
        if hit.get("declaration").and_then(Value::as_bool) == Some(true) {
            declaration = hit;
            continue;
        }

        let Some(line) = hit.get("line").and_then(Value::as_u64) else {
            unclassified.push(hit);
            continue;
        };

        let Some(known) = context.get(file) else {
            unclassified.push(hit);
            continue;
        };

        let Some(spans) = known.test_spans.as_deref() else {
            unclassified.push(hit);
            continue;
        };

        let mut hit = hit;
        match innermost(spans, line) {
            Some(span) => {
                hit["test"] = json!(span.label);
                tests.push(hit);
            }
            None => {
                if let Some(item) = enclosing_item(&known.symbols, line) {
                    hit["item"] = json!(item);
                }
                production.push(hit);
            }
        }
    }

    let mut warnings = Vec::new();
    if !unclassified.is_empty() {
        warnings.push(json!(format!(
            "{} reference(s) could not be placed on either side of the test/production split, \
             because rust-analyzer would not say which parts of their file it would run. They are \
             listed under `unclassified`; treat the production count as a lower bound.",
            unclassified.len()
        )));
    }
    if production.is_empty() && tests.is_empty() && unclassified.is_empty() {
        warnings.push(json!(
            "Nothing refers to this symbol anywhere in the loaded workspace. That is a real \
             answer, not an empty one -- the position was checked against rust-analyzer before \
             the search -- but it is about reference edges only: a copy of this code pasted \
             elsewhere has no edge to it and would not appear here. A repo-wide text sweep for \
             the name is the other half of that question."
        ));
    }

    let mut answer = json!({
        "target": target,
        "complete": unclassified.is_empty(),
        "production": { "count": production.len(), "references": production },
        "tests": { "count": tests.len(), "references": tests },
        "files_touched": files_touched,
        "warnings": warnings,
    });

    if !declaration.is_null() {
        answer["declaration"] = declaration;
    }
    if !unclassified.is_empty() {
        answer["unclassified"] = json!(unclassified);
    }
    // Implementations are the half of a blast radius that has nothing to do with call sites:
    // change a trait method and every impl of it has to change too, whether or not anything calls
    // it. Empty for anything that is not a trait, so it is only reported when there is something
    // to report.
    if implementations
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        answer["implementations"] = implementations.clone();
    }

    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape rust-analyzer sends for `src/uri.rs`, measured against the running server:
    /// a `test-mod` runnable spanning the whole `#[cfg(test)]` module, a `test` runnable per
    /// function inside it, and two package-level runnables carrying no location at all.
    fn runnables() -> Value {
        json!([
            {
                "kind": "cargo",
                "label": "test-mod uri::tests",
                "location": {
                    "targetRange": {
                        "start": {"line": 121, "character": 0},
                        "end": {"line": 261, "character": 1}
                    }
                }
            },
            {
                "kind": "cargo",
                "label": "test uri::tests::unix_paths_round_trip",
                "location": {
                    "targetRange": {
                        "start": {"line": 196, "character": 4},
                        "end": {"line": 211, "character": 5}
                    }
                }
            },
            {"kind": "cargo", "label": "cargo check -p rust-analyzer-mcp --all-targets"},
            {"kind": "cargo", "label": "cargo test -p rust-analyzer-mcp --all-targets"},
        ])
    }

    #[test]
    fn package_wide_runnables_are_not_spans() {
        let spans = test_spans(&runnables());
        assert_eq!(spans.len(), 2, "the two `cargo …` entries have no range");
        assert!(spans.iter().all(|span| span.end_line > span.start_line));
    }

    #[test]
    fn running_a_binary_is_not_a_test() {
        let spans = test_spans(&json!([{
            "label": "run main",
            "location": {"targetRange": {
                "start": {"line": 3, "character": 0},
                "end": {"line": 9, "character": 1}
            }}
        }]));
        assert!(spans.is_empty());
    }

    #[test]
    fn a_bench_is_not_production() {
        let spans = test_spans(&json!([{
            "label": "bench parse",
            "location": {"targetRange": {
                "start": {"line": 1, "character": 0},
                "end": {"line": 5, "character": 1}
            }}
        }]));
        assert_eq!(
            spans.len(),
            1,
            "a benchmark constrains the production API the way a test does"
        );
    }

    /// This assertion was inverted until 2026-09-21, and the test asserting the opposite is how
    /// the bug shipped: a doctest *is* a test, so counting its range as a test span reads as
    /// obviously right, and a hand-written fixture will give it whatever range the author
    /// expects. The range is the whole of the difference -- 0 to 7 below is what the running
    /// server actually returns for `doctest documented_caller`, and it is the function itself,
    /// not the four lines of fenced code inside its doc comment.
    #[test]
    fn a_doctests_range_is_the_documented_item_so_it_is_not_a_test_span() {
        let spans = test_spans(&json!([{
            "label": "doctest documented_caller",
            "location": {"targetRange": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 7, "character": 1}
            }}
        }]));
        assert!(
            spans.is_empty(),
            "a doctest's span covers the documented function's own body, so every hit it could \
             capture is production"
        );
    }

    #[test]
    fn the_narrower_test_wins_over_the_module_containing_it() {
        let spans = test_spans(&runnables());
        let named = innermost(&spans, 200).map(|span| span.label.as_str());
        assert_eq!(named, Some("test uri::tests::unix_paths_round_trip"));
    }

    #[test]
    fn a_line_in_the_test_module_but_in_no_test_is_still_a_test_line() {
        // A helper `fn` inside `#[cfg(test)] mod tests` has no runnable of its own, and is test
        // code all the same. Without the module span it would be filed under production, which is
        // the whole failure this partition exists to avoid.
        let spans = test_spans(&runnables());
        let named = innermost(&spans, 130).map(|span| span.label.as_str());
        assert_eq!(named, Some("test-mod uri::tests"));
    }

    #[test]
    fn a_line_outside_every_test_is_production() {
        let spans = test_spans(&runnables());
        assert!(innermost(&spans, 40).is_none());
    }

    fn symbols() -> Value {
        json!([{
            "name": "Calculator",
            "kind": 23,
            "range": {"start": {"line": 10, "character": 0}, "end": {"line": 40, "character": 1}},
            "selectionRange": {
                "start": {"line": 10, "character": 7},
                "end": {"line": 10, "character": 17}
            },
            "children": [{
                "name": "add",
                "kind": 6,
                "range": {
                    "start": {"line": 14, "character": 4},
                    "end": {"line": 20, "character": 5}
                },
                "selectionRange": {
                    "start": {"line": 15, "character": 11},
                    "end": {"line": 15, "character": 14}
                },
                "children": []
            }]
        }])
    }

    #[test]
    fn a_hit_is_named_by_the_deepest_item_holding_it() {
        assert_eq!(
            enclosing_item(&symbols(), 17).as_deref(),
            Some("Calculator::add")
        );
    }

    #[test]
    fn a_hit_between_the_children_is_named_by_the_parent() {
        assert_eq!(
            enclosing_item(&symbols(), 12).as_deref(),
            Some("Calculator")
        );
    }

    #[test]
    fn a_hit_in_no_item_is_named_by_nothing() {
        assert_eq!(enclosing_item(&symbols(), 90), None);
    }

    fn hit(file: &str, line: u64) -> Value {
        json!({"file": file, "line": line, "character": 8, "text": "    greet(name)"})
    }

    fn context() -> BTreeMap<String, FileContext> {
        BTreeMap::from([(
            "src/uri.rs".to_string(),
            FileContext {
                test_spans: Some(test_spans(&runnables())),
                symbols: symbols(),
            },
        )])
    }

    #[test]
    fn callers_in_one_file_land_on_both_sides_of_the_split() {
        // The measurement this whole module exists for: production code and its `#[cfg(test)]`
        // tests share a file, so a split by filename puts all four hits on one side. Every count
        // below is wrong under such a rule, in both directions.
        let annotated = json!({"locations": [
            hit("src/uri.rs", 17),
            hit("src/uri.rs", 200),
            hit("src/uri.rs", 130),
        ]});

        let answer = assemble(
            json!({"name": "absolute"}),
            &annotated,
            &context(),
            &json!([]),
        );

        assert_eq!(answer["production"]["count"], 1);
        assert_eq!(answer["tests"]["count"], 2);
        assert_eq!(answer["complete"], true);
        assert_eq!(
            answer["production"]["references"][0]["item"],
            "Calculator::add"
        );
        assert_eq!(
            answer["tests"]["references"][0]["test"],
            "test uri::tests::unix_paths_round_trip"
        );
    }

    /// The minimal repro that established the doctest bug, in the shape `assemble` sees it: two
    /// callers of one private helper, one carrying a doctest and the other -- the control that
    /// says the variable is the doctest rather than merely being documented -- carrying rustdoc
    /// with no code fence.
    fn documented_callers() -> BTreeMap<String, FileContext> {
        let runnables = json!([{
            "label": "doctest documented_caller",
            "location": {"targetRange": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 7, "character": 1}
            }}
        }]);
        let symbols = json!([
            {
                "name": "documented_caller",
                "kind": 12,
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 7, "character": 1}
                },
                "selectionRange": {
                    "start": {"line": 5, "character": 7},
                    "end": {"line": 5, "character": 24}
                },
                "children": []
            },
            {
                "name": "undocumented_caller",
                "kind": 12,
                "range": {
                    "start": {"line": 9, "character": 0},
                    "end": {"line": 14, "character": 1}
                },
                "selectionRange": {
                    "start": {"line": 12, "character": 7},
                    "end": {"line": 12, "character": 26}
                },
                "children": []
            }
        ]);

        BTreeMap::from([(
            "src/lib.rs".to_string(),
            FileContext {
                test_spans: Some(test_spans(&runnables)),
                symbols,
            },
        )])
    }

    #[test]
    fn a_caller_that_carries_a_doctest_is_still_a_production_caller() {
        // MEASURED both ways 2026-09-21 against the running server. Before the fix this answered
        // production 1 / tests 1, the documented caller filed under `doctest documented_caller`
        // though its hit is in the function body. On a crate whose standard is to document its
        // public surface, a helper called only from documented functions answered
        // `production: {count: 0}` with `complete: true` -- which reads as dead code.
        let annotated = json!({"locations": [hit("src/lib.rs", 6), hit("src/lib.rs", 13)]});

        let answer = assemble(
            json!({"name": "helper"}),
            &annotated,
            &documented_callers(),
            &json!([]),
        );

        assert_eq!(answer["production"]["count"], 2, "{answer}");
        assert_eq!(
            answer["tests"]["count"], 0,
            "no reference edge reaches doctest code, so no hit here belongs to one: {answer}"
        );

        let named: Vec<&str> = answer["production"]["references"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|hit| hit["item"].as_str())
            .collect();
        assert_eq!(named, ["documented_caller", "undocumented_caller"]);
    }

    #[test]
    fn the_declaration_is_counted_as_neither() {
        let mut declaration = hit("src/uri.rs", 17);
        declaration["declaration"] = json!(true);
        let annotated = json!({"locations": [declaration, hit("src/uri.rs", 200)]});

        let answer = assemble(
            json!({"name": "absolute"}),
            &annotated,
            &context(),
            &json!([]),
        );

        assert_eq!(answer["production"]["count"], 0);
        assert_eq!(answer["tests"]["count"], 1);
        assert_eq!(answer["declaration"]["line"], 17);
    }

    #[test]
    fn a_file_that_would_not_say_is_reported_as_unknown_not_as_production() {
        // The failure mode being guarded: a file whose runnables could not be had must not have
        // its hits quietly filed under production, where they would read as confirmed callers.
        let context = BTreeMap::from([(
            "src/uri.rs".to_string(),
            FileContext {
                test_spans: None,
                symbols: symbols(),
            },
        )]);
        let annotated = json!({"locations": [hit("src/uri.rs", 200)]});

        let answer = assemble(
            json!({"name": "absolute"}),
            &annotated,
            &context,
            &json!([]),
        );

        assert_eq!(answer["production"]["count"], 0);
        assert_eq!(answer["tests"]["count"], 0);
        assert_eq!(answer["complete"], false);
        assert_eq!(answer["unclassified"][0]["line"], 200);
        assert!(!answer["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_symbol_nothing_refers_to_says_so_rather_than_saying_nothing() {
        let answer = assemble(
            json!({"name": "absolute"}),
            &json!({"locations": []}),
            &context(),
            &json!([]),
        );

        assert_eq!(answer["complete"], true);
        assert_eq!(answer["production"]["count"], 0);
        let warning = answer["warnings"][0].as_str().unwrap();
        assert!(warning.contains("copy"), "the text-sweep half is named");
    }

    /// Measured against the running server: the first fenced block of a hover is the module path
    /// and the signature comes after it, so taking only the first reports the module where the
    /// symbol was wanted.
    #[test]
    fn a_hover_is_reduced_to_its_code_blocks_signature_included() {
        let hover = json!({"contents": {
            "kind": "markdown",
            "value": "```rust\nrust_analyzer_mcp::uri\n```\n\n```rust\npub fn absolute(path: &Path) -> PathBuf\n```\n\n---\n\nProse nobody needs here."
        }});
        assert_eq!(
            hover_signature(&hover).as_deref(),
            Some("rust_analyzer_mcp::uri\npub fn absolute(path: &Path) -> PathBuf")
        );
    }

    #[test]
    fn the_prose_under_a_hover_is_left_where_it_is_written() {
        let hover = json!({"contents": {
            "kind": "markdown",
            "value": "```rust\npub fn absolute(path: &Path) -> PathBuf\n```\n\n---\n\nParagraphs of rustdoc.\n\n```sh\nnot rust\n```"
        }});
        let signature = hover_signature(&hover).unwrap();
        assert!(!signature.contains("Paragraphs"));
        assert!(!signature.contains("not rust"), "only rust blocks are kept");
    }

    #[test]
    fn a_hover_that_said_nothing_is_none_rather_than_an_empty_string() {
        // The distinction the tool refuses to search past: `null` and `""` both mean the position
        // is not on a symbol, and neither may be reported as a signature.
        assert_eq!(hover_signature(&Value::Null), None);
        assert_eq!(
            hover_signature(&json!({"contents": {"kind": "markdown", "value": "  \n "}})),
            None
        );
    }

    #[test]
    fn impls_are_reported_only_when_there_are_any() {
        let annotated = json!({"locations": [hit("src/uri.rs", 17)]});

        let none = assemble(json!({}), &annotated, &context(), &json!([]));
        assert!(none.get("implementations").is_none());

        let some = assemble(
            json!({}),
            &annotated,
            &context(),
            &json!([{"uri": "file:///x.rs"}]),
        );
        assert_eq!(some["implementations"].as_array().unwrap().len(), 1);
    }
}
