use crate::protocol::mcp::{ToolAnnotations, ToolDefinition};
use serde_json::{json, Value};

/// The name a person reads for each tool, and whether the tool leaves everything as it found it.
///
/// Kept apart from the definitions because it is a table: read down it and the read-only column
/// is checkable at a glance, which is the property that matters about it. Written into the
/// definitions it would be one line lost in each of twenty-one screens of schema.
///
/// `format`, `rename` and `ssr` are read-only because they return an edit and write nothing --
/// the decision to change a file stays with the caller. `set_workspace` is the one that is not:
/// it moves the default workspace for everyone sharing the server.
const ANNOTATIONS: &[(&str, &str, bool)] = &[
    ("rust_analyzer_hover", "Hover", true),
    ("rust_analyzer_definition", "Go to definition", true),
    ("rust_analyzer_references", "Find references", true),
    ("rust_analyzer_blast_radius", "Blast radius", true),
    ("rust_analyzer_completion", "Completions", true),
    ("rust_analyzer_symbols", "File symbols", true),
    (
        "rust_analyzer_workspace_symbols",
        "Find symbol by name",
        true,
    ),
    (
        "rust_analyzer_type_definition",
        "Go to type definition",
        true,
    ),
    ("rust_analyzer_implementation", "Find implementations", true),
    ("rust_analyzer_expand_macro", "Expand macro", true),
    ("rust_analyzer_related_tests", "Related tests", true),
    ("rust_analyzer_runnables", "Runnable commands", true),
    ("rust_analyzer_ssr", "Structural search and replace", true),
    ("rust_analyzer_incoming_calls", "Incoming calls", true),
    ("rust_analyzer_outgoing_calls", "Outgoing calls", true),
    ("rust_analyzer_format", "Format", true),
    ("rust_analyzer_code_actions", "Code actions", true),
    ("rust_analyzer_rename", "Rename", true),
    ("rust_analyzer_diagnostics", "Diagnostics", true),
    (
        "rust_analyzer_workspace_diagnostics",
        "Workspace diagnostics",
        true,
    ),
    ("rust_analyzer_set_workspace", "Set workspace", false),
];

/// The descriptions of the parameters nearly every tool takes.
///
/// One tool's listing is read by a model, in full, at the start of every session that holds this
/// server -- so a sentence here is not written once, it is paid for on every call to every seat
/// on every machine. `workspace_path` alone appears on twenty tools: each word of it costs
/// twenty times what it reads like.
const WORKSPACE: &str = "This call's workspace; others' default untouched. Needs Cargo.toml.";
const FILE: &str = "Rust file: relative, absolute, or file:// URI.";
const LINE: &str = "0-based.";
const CHARACTER: &str = "0-based.";

/// The schema of a tool that asks about one position in one file, which is most of them.
fn at_position() -> Value {
    json!({
        "type": "object",
        "properties": {
            "workspace_path": { "type": "string", "description": WORKSPACE },
            "file_path": { "type": "string", "description": FILE },
            "line": { "type": "number", "description": LINE },
            "character": { "type": "number", "description": CHARACTER }
        },
        "required": ["file_path", "line", "character"]
    })
}

/// The schema of a tool that asks about a whole file.
fn in_file() -> Value {
    json!({
        "type": "object",
        "properties": {
            "workspace_path": { "type": "string", "description": WORKSPACE },
            "file_path": { "type": "string", "description": FILE }
        },
        "required": ["file_path"]
    })
}

fn tool(name: &str, description: &str, input_schema: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        annotations: None,
    }
}

pub fn get_tools() -> Vec<ToolDefinition> {
    let mut tools = definitions();
    for tool in &mut tools {
        tool.annotations = ANNOTATIONS
            .iter()
            .find(|(name, _, _)| *name == tool.name)
            .map(|(_, title, read_only)| ToolAnnotations {
                title: (*title).to_string(),
                read_only_hint: *read_only,
            });
    }
    tools
}

fn definitions() -> Vec<ToolDefinition> {
    vec![
        tool(
            "rust_analyzer_hover",
            "Type signature and docs at a position. Also the coordinate check: a signature back \
             means the position is on a symbol, so an empty references/implementation there is \
             real; nothing back means the position is wrong.",
            at_position(),
        ),
        tool(
            "rust_analyzer_definition",
            "Where the symbol at a position is defined. May land outside the workspace, in a \
             dependency's or std's own sources.",
            at_position(),
        ),
        tool(
            "rust_analyzer_references",
            "Every mention of the symbol at a position, each with the line of source at it. \
             Returns {count, locations[{file, line, character, text, declaration?}]}. `count` \
             INCLUDES the declaration, marked `declaration: true` -- two call sites report 3. Not \
             a caller count. The mark is absent if the declaration resolves outside the \
             workspace, and then there is no extra hit to subtract. For \"what breaks if I change \
             this\", use blast_radius.",
            at_position(),
        ),
        tool(
            "rust_analyzer_blast_radius",
            "What breaks if the symbol at a position changes -- what a reference list usually \
             stands in for, in one call. Returns {target, complete, production{count, \
             references}, tests{count, references}, files_touched, warnings} plus declaration?, \
             unclassified?, implementations?. Production hits are named by the item holding them, \
             test hits by the test to rerun, and implementations are the impls that must change \
             whether or not anything calls them. The test split comes from runnables, not \
             filenames, so a `#[cfg(test)] mod tests` in the file it tests lands on the test \
             side. The declaration counts as NEITHER side. Hovers first, so empty is never a \
             wrong coordinate. Read `complete` first: false puts hits under `unclassified` and \
             makes both counts lower bounds. Blind to COPIES, which have no reference edge -- a \
             text sweep is the other half.",
            at_position(),
        ),
        tool(
            "rust_analyzer_completion",
            "Completion candidates at a position.",
            at_position(),
        ),
        tool(
            "rust_analyzer_symbols",
            "Every symbol in one file, as nested DocumentSymbols. Take a position from \
             `selectionRange.start`, which is the identifier -- `range.start` is the doc comment, \
             lines earlier. `detail` carries the signature. Syntactic, so it answers before the \
             index is ready and for a file no workspace has loaded.",
            in_file(),
        ),
        tool(
            "rust_analyzer_implementation",
            "What implements the trait, or the trait method, at a position. No text search can \
             answer this even in principle: which type an impl belongs to is decided by dispatch, \
             not by anything written at the position. On a trait, the implementing types; on a \
             trait method, the bodies implementing it.",
            at_position(),
        ),
        tool(
            "rust_analyzer_type_definition",
            "Definition of the TYPE of the thing at a position, not of the thing. On a variable \
             or field this lands on the type it holds; `definition` would land on its own \
             declaration.",
            at_position(),
        ),
        tool(
            "rust_analyzer_incoming_calls",
            "The FUNCTIONS that call the function at a position, each with its call sites inside \
             them. References lists every mention and leaves you to work out what contains each \
             one; this groups them. Returns {item, calls}, where `item` is what the position \
             resolved to. Errors if the position is not on something callable.",
            at_position(),
        ),
        tool(
            "rust_analyzer_outgoing_calls",
            "The functions called BY the function at a position. Returns {item, calls}. Expect \
             hits outside the workspace: a call into a dependency or std resolves to that crate's \
             sources.",
            at_position(),
        ),
        tool(
            "rust_analyzer_expand_macro",
            "The code the macro call at a position expands to. Macro-generated code is in no \
             file, so reading, grep and structural search all miss it, and hover does not show it \
             either. Use it when a type, method or impl seems to come from nowhere. Position goes \
             on the macro's name; errors if nothing there expands.",
            at_position(),
        ),
        tool(
            "rust_analyzer_related_tests",
            "The tests that exercise the symbol at a position. Worked out from calls, so a test \
             reaching it only through a trait object or a macro may not appear: empty means none \
             was found, not that none exists.",
            at_position(),
        ),
        tool(
            "rust_analyzer_runnables",
            "The exact cargo commands that run what is in a file -- tests, binary, doctests -- \
             with the args and env rust-analyzer would use, so a test filter or package name need \
             not be guessed. Omit the position for the whole file; give BOTH line and character \
             to narrow to one item.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": WORKSPACE },
                    "file_path": { "type": "string", "description": FILE },
                    "line": { "type": "number", "description": "0-based line. Optional, but only with `character`: line alone is an error, not a whole-file answer." },
                    "character": { "type": "number", "description": "0-based character. Optional, but only with `line`." }
                },
                "required": ["file_path"]
            }),
        ),
        tool(
            "rust_analyzer_workspace_symbols",
            "Name to position, across the workspace -- the only way here to turn a name into the \
             file, line and character the other tools need. DISCOVERY, NOT A CENSUS: matched \
             fuzzily (letters need only appear in order), scored, and capped at a few dozen. A \
             name missing from the results is NOT evidence it does not exist, and unrelated \
             symbols scoring on scattered letters are expected. Never count from it. An empty \
             query returns nothing.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": WORKSPACE },
                    "query": { "type": "string", "description": "Name, or part of one." }
                },
                "required": ["query"]
            }),
        ),
        tool(
            "rust_analyzer_ssr",
            "Structural search and replace whose patterns are resolved by TYPE, not by text. \
             `query` is rust-analyzer's `pattern ==>> replacement` form and `$name` binds an \
             expression: `Foo::bar($a) ==>> Foo::baz($a)`. The paths are resolved from the module \
             at the position, so it matches that `Foo` alone where a syntactic rewriter matches \
             every type spelled `Foo` in the workspace. WRITES NOTHING: the answer is a workspace \
             edit handed back, so an unfamiliar query is safe to ask and read. `parse_only` \
             checks a pattern is well formed without searching.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": WORKSPACE },
                    "query": { "type": "string", "description": "`pattern ==>> replacement`; `$name` binds an expression." },
                    "file_path": { "type": "string", "description": "Rust file whose module the pattern's paths resolve from." },
                    "line": { "type": "number", "description": "0-based line to resolve from. Defaults to the top of the file." },
                    "character": { "type": "number", "description": "0-based character. Defaults to the top of the file." },
                    "parse_only": { "type": "boolean", "description": "Check the query parses, without searching. Default false." }
                },
                "required": ["query", "file_path"]
            }),
        ),
        tool(
            "rust_analyzer_format",
            "The rustfmt edits for a file. RETURNS EDITS AND WRITES NOTHING -- the file is \
             unchanged until you apply them. An empty answer means it is already formatted.",
            in_file(),
        ),
        tool(
            "rust_analyzer_code_actions",
            "The code actions offered for a range: quickfixes, refactors, assists. Returns the \
             actions and applies none. Empty means none is offered there, which is ordinary.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": WORKSPACE },
                    "file_path": { "type": "string", "description": FILE },
                    "line": { "type": "number", "description": "0-based start line." },
                    "character": { "type": "number", "description": "0-based start character." },
                    "end_line": { "type": "number", "description": "0-based end line." },
                    "end_character": { "type": "number", "description": "0-based end character." }
                },
                "required": ["file_path", "line", "character", "end_line", "end_character"]
            }),
        ),
        tool(
            "rust_analyzer_rename",
            "Rename the symbol at a position across the workspace. RETURNS EDITS AND WRITES \
             NOTHING: {applied: false, old_name, new_name, position_encoding, summary, changes, \
             file_operations, workspace_edit}. Each edit carries `byte_range` and `old_text`, and \
             a file's edits are ordered last-first so applying them in order needs no \
             re-offsetting. Renaming a module renames its file, under `file_operations`. Errors \
             if nothing is renameable there, the symbol comes from a dependency, or the workspace \
             is still loading -- a rename worked out early can miss references.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": WORKSPACE },
                    "file_path": { "type": "string", "description": FILE },
                    "line": { "type": "number", "description": LINE },
                    "character": { "type": "number", "description": CHARACTER },
                    "new_name": { "type": "string", "description": "New name. For a lifetime, include the apostrophe." }
                },
                "required": ["file_path", "line", "character", "new_name"]
            }),
        ),
        tool(
            "rust_analyzer_set_workspace",
            "Move the default workspace for every later call, and shut down the rust-analyzer of \
             every other workspace this server holds. Respawns and reindexes, which costs seconds \
             and gigabytes on a large repo, and it is shared state: it moves the default out from \
             under everyone else on this server. Prefer `workspace_path` on the call itself, \
             which answers from the named workspace and moves nobody's default. Refuses a \
             directory with no Cargo.toml.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": "New default workspace root: path or file:// URI. Must contain Cargo.toml." }
                },
                "required": ["workspace_path"]
            }),
        ),
        tool(
            "rust_analyzer_diagnostics",
            "Errors, warnings and hints for one file: {summary, diagnostics[], note?}. `summary` \
             IS the fault count -- rust-analyzer and rustc both report a fault and it is counted \
             once -- and each entry carries `sources` with the compiler's wording preferred. A \
             `note` means the report is incomplete and the counts are a floor. A superset of \
             `cargo check` rather than a second opinion, so cargo remains the authority on \
             whether the build is broken.",
            in_file(),
        ),
        tool(
            "rust_analyzer_workspace_diagnostics",
            "Errors and warnings across the whole workspace, from one cargo check -- no file need \
             be open. Returns {workspace, summary, complete, files{}, note?}. READ `complete` \
             FIRST: a zero means the workspace is clean only when it is true; with false nothing \
             was analysed and the zero means nothing.",
            json!({
                "type": "object",
                "properties": {
                    "workspace_path": { "type": "string", "description": WORKSPACE }
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table and the definitions are two lists of the same tools, and nothing but this keeps
    /// them the same list. Both directions matter: a tool added without a row shows its wire name
    /// and is assumed to change things, and a row left behind by a rename is a label for a tool
    /// that no longer exists.
    #[test]
    fn every_tool_is_annotated_and_every_annotation_names_a_tool() {
        let tools = get_tools();

        let unannotated: Vec<&str> = tools
            .iter()
            .filter(|tool| tool.annotations.is_none())
            .map(|tool| tool.name.as_str())
            .collect();
        assert!(
            unannotated.is_empty(),
            "these tools would show their wire name and be assumed to write: {unannotated:?}"
        );

        let stale: Vec<&str> = ANNOTATIONS
            .iter()
            .map(|(name, _, _)| *name)
            .filter(|name| !tools.iter().any(|tool| tool.name == *name))
            .collect();
        assert!(stale.is_empty(), "these name no tool: {stale:?}");
    }

    /// A read-only hint is a claim a client acts on, so the one tool making the opposite claim is
    /// worth pinning: `set_workspace` moves the default workspace for everyone sharing the
    /// server, and a run of this file that quietly makes everything read-only would not otherwise
    /// fail.
    #[test]
    fn the_tool_that_changes_something_says_so() {
        let writes: Vec<String> = get_tools()
            .iter()
            .filter(|tool| {
                tool.annotations
                    .as_ref()
                    .is_some_and(|annotations| !annotations.read_only_hint)
            })
            .map(|tool| tool.name.clone())
            .collect();

        assert_eq!(writes, ["rust_analyzer_set_workspace"]);
    }

    /// The listing goes to a model in full at the start of every session holding this server, so
    /// its size is paid per seat rather than per repo. The pressure on it is one way: every trap
    /// worth warning about argues for one more sentence, and nothing argues back.
    ///
    /// Measured per tool rather than in total, so that adding a tool does not put the squeeze on
    /// the ones already here -- the thing worth catching is a description growing back into
    /// prose, not the tool count going up. It was 869 bytes a tool when the descriptions were
    /// narrative and the schemas repeated themselves; it is a little under 700 now.
    ///
    /// The headroom is deliberately thin. At a round 800 this passed with six hundred words of
    /// filler pasted into a description -- twenty-one tools' worth of slack absorbs one essay
    /// without noticing, which is the whole failure it exists to catch. Raising the number is a
    /// fair thing to do when a tool genuinely needs more; doing it to make a red test go green
    /// is how it becomes decoration again, so say what was measured.
    #[test]
    fn the_listing_stays_terse_enough_to_send_every_session() {
        let tools = get_tools();
        let sizes: Vec<(usize, &str)> = tools
            .iter()
            .map(|tool| {
                (
                    tool.description.len() + tool.input_schema.to_string().len(),
                    tool.name.as_str(),
                )
            })
            .collect();

        let listing: usize = sizes.iter().map(|(size, _)| size).sum();
        let average = listing / tools.len();

        let mut biggest = sizes.clone();
        biggest.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
        assert!(
            average <= 715,
            "the tool listing averages {average} bytes over {} tools ({listing} total) and goes \
             out on every session. Trim a description rather than raising this; the heaviest are \
             {:?}",
            tools.len(),
            &biggest[..3]
        );
    }
}
