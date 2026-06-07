//! Event-trigger generators (v0.4.0 coverage).
//!
//! Event triggers are database-global objects that fire a `RETURNS
//! event_trigger` function on DDL events. The soak previously generated no
//! functions and no event triggers, so this module adds both: each generated
//! event trigger is paired with a dedicated PL/pgSQL function that lives in a
//! managed schema. Functions and event triggers are returned together so the
//! caller can append the functions to `catalog.functions` and set
//! `catalog.event_triggers` in lockstep, keeping every `EventTrigger.function`
//! reference pointing at a real function.
//!
//! ## Round-trip correctness
//!
//! The catalog reader rebuilds a function's body by re-parsing the read-back
//! body through [`pgevolve_core::parse::builder::plpgsql::parse_routine_body`].
//! To guarantee the generated function's canonical body equals the read-back
//! one, this generator builds its body through the *same* helper with the same
//! input (`BEGIN\nEND`, PL/pgSQL). The `cost`/`rows` fields are left `None`
//! because canon's `filter_pg_defaults` normalizes PG's default function
//! cost/rows to `None`, so the reader's values normalize to match.

#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use proptest::prelude::*;

use pgevolve_core::identifier::{Identifier, QualifiedName};
use pgevolve_core::ir::event_trigger::{EventTrigger, EventTriggerEnabled, EventTriggerEvent};
use pgevolve_core::ir::function::{
    Function, FunctionLanguage, NormalizedArgTypes, ParallelSafety, ReturnType, SecurityMode,
    Volatility,
};
use pgevolve_core::parse::builder::plpgsql::parse_routine_body;
use pgevolve_core::parse::error::SourceLocation;

/// Command-tag pool for the unrestricted events (`ddl_command_start`,
/// `ddl_command_end`, `sql_drop`). Canon sorts + dedupes the chosen subset,
/// so the generator may pass any subsequence.
const TAG_POOL: &[&str] = &["CREATE TABLE", "ALTER TABLE", "DROP TABLE"];

/// Command-tag pool for the `table_rewrite` event.
///
/// `table_rewrite` only fires on commands that rewrite a table's heap, so PG
/// rejects `CREATE TABLE` / `DROP TABLE` in its `WHEN TAG` filter at
/// `CREATE EVENT TRIGGER` time with `0A000` ("event triggers are not supported
/// for CREATE TABLE"). Of our pool only `ALTER TABLE` is valid (verified
/// empirically against PG 16). Keeping this restriction in the generator means
/// the soak never emits a catalog PG refuses to build. See
/// <https://www.postgresql.org/docs/current/event-trigger-definition.html>.
const TABLE_REWRITE_TAG_POOL: &[&str] = &["ALTER TABLE"];

/// The command-tag pool valid for a given event.
const fn tag_pool_for(event: EventTriggerEvent) -> &'static [&'static str] {
    match event {
        EventTriggerEvent::TableRewrite => TABLE_REWRITE_TAG_POOL,
        EventTriggerEvent::DdlCommandStart
        | EventTriggerEvent::DdlCommandEnd
        | EventTriggerEvent::SqlDrop => TAG_POOL,
    }
}

/// The four DDL events an event trigger can fire on.
const EVENTS: &[EventTriggerEvent] = &[
    EventTriggerEvent::DdlCommandStart,
    EventTriggerEvent::DdlCommandEnd,
    EventTriggerEvent::SqlDrop,
    EventTriggerEvent::TableRewrite,
];

/// The four fire states.
const ENABLED: &[EventTriggerEnabled] = &[
    EventTriggerEnabled::Enabled,
    EventTriggerEnabled::Disabled,
    EventTriggerEnabled::Replica,
    EventTriggerEnabled::Always,
];

/// Build the dedicated `RETURNS event_trigger` PL/pgSQL function backing one
/// event trigger. The body is produced via [`parse_routine_body`] so its
/// canonical form matches exactly what the catalog reader rebuilds.
fn build_event_trigger_function(qname: QualifiedName) -> Function {
    let args = vec![];
    let arg_types_normalized = NormalizedArgTypes::from_args(&args);
    // Synthetic source location — the generator is not parsing a real file.
    let loc = SourceLocation::new(PathBuf::from("<generated>"), 1, 1);
    // `BEGIN\nEND` yields an empty dep list; mirror the reader exactly.
    let (body, body_dependencies, _commits) =
        parse_routine_body("BEGIN\nEND", FunctionLanguage::PlPgSql, &qname, &loc)
            .expect("BEGIN\\nEND is a valid PL/pgSQL body");
    Function {
        qname,
        args,
        arg_types_normalized,
        return_type: ReturnType::EventTrigger,
        language: FunctionLanguage::PlPgSql,
        body,
        body_dependencies,
        volatility: Volatility::Volatile,
        strict: false,
        security: SecurityMode::Invoker,
        parallel: ParallelSafety::Unsafe,
        leakproof: false,
        cost: None,
        rows: None,
        comment: None,
        owner: None,
        grants: vec![],
    }
}

/// Generate 0–3 event triggers, each paired with its dedicated
/// `RETURNS event_trigger` function.
///
/// Returns `(functions, event_triggers)`: the functions must be appended to
/// `catalog.functions` and the event triggers assigned to
/// `catalog.event_triggers` *before* canonicalization. Names are unique:
/// event-trigger names are globally distinct (`et_0`, `et_1`, …) and the
/// backing function names are distinct within their drawn schema
/// (`et_fn_0`, `et_fn_1`, …).
///
/// Each function's schema is drawn from the managed `schema_pool`. If the pool
/// is empty (no schemas to host a function), no event triggers are generated.
pub(super) fn arb_event_triggers(
    schema_pool: Vec<Identifier>,
) -> BoxedStrategy<(Vec<Function>, Vec<EventTrigger>)> {
    if schema_pool.is_empty() {
        return Just((Vec::new(), Vec::new())).boxed();
    }
    (0usize..=3usize)
        .prop_flat_map(move |count| {
            let schema_pool = schema_pool.clone();
            // Per event trigger: a schema index, an event, an enabled state,
            // and a subsequence of the tag pool.
            let per_et: Vec<_> = (0..count)
                .map(move |idx| {
                    let schema_pool = schema_pool.clone();
                    (
                        proptest::sample::select(schema_pool),
                        proptest::sample::select(EVENTS),
                        proptest::sample::select(ENABLED),
                    )
                        .prop_flat_map(move |(schema, event, enabled)| {
                            // The valid command-tag pool depends on the event:
                            // `table_rewrite` only accepts rewrite-capable tags
                            // (PG rejects the others at CREATE time).
                            let pool = tag_pool_for(event).to_vec();
                            let pool_len = pool.len();
                            (
                                Just(schema),
                                Just(event),
                                Just(enabled),
                                proptest::sample::subsequence(pool, 0..=pool_len),
                            )
                        })
                        .prop_map(move |(schema, event, enabled, tags)| {
                            let fn_qname = QualifiedName::new(
                                schema,
                                Identifier::from_unquoted(&format!("et_fn_{idx}")).unwrap(),
                            );
                            let function = build_event_trigger_function(fn_qname.clone());
                            let event_trigger = EventTrigger {
                                name: Identifier::from_unquoted(&format!("et_{idx}")).unwrap(),
                                event,
                                tag_filter: tags.iter().map(|t| (*t).to_string()).collect(),
                                function: fn_qname,
                                enabled,
                                owner: None,
                                comment: None,
                            };
                            (function, event_trigger)
                        })
                })
                .collect();
            per_et
        })
        .prop_map(|pairs| {
            let mut functions = Vec::with_capacity(pairs.len());
            let mut event_triggers = Vec::with_capacity(pairs.len());
            for (f, et) in pairs {
                functions.push(f);
                event_triggers.push(et);
            }
            (functions, event_triggers)
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    use pgevolve_core::identifier::Identifier;

    use super::arb_event_triggers;

    fn id(s: &str) -> Identifier {
        Identifier::from_unquoted(s).unwrap()
    }

    /// Every generated event trigger must reference a function that is also
    /// returned, and that function's schema must come from the supplied pool.
    #[test]
    fn each_event_trigger_has_its_function() {
        let pool = vec![id("app"), id("public")];
        let strategy = arb_event_triggers(pool.clone());
        let mut runner = TestRunner::default();
        for _ in 0..256 {
            let (funcs, ets) = strategy.new_tree(&mut runner).unwrap().current();
            assert_eq!(
                funcs.len(),
                ets.len(),
                "each event trigger must be paired with exactly one function",
            );
            for et in &ets {
                assert!(
                    funcs.iter().any(|f| f.qname == et.function),
                    "event trigger '{}' references a missing function '{}'",
                    et.name.as_str(),
                    et.function.render_sql(),
                );
                assert!(
                    pool.contains(&et.function.schema),
                    "function schema '{}' not drawn from the pool",
                    et.function.schema.as_str(),
                );
            }
        }
    }

    /// Across many draws, at least some catalogs must produce a non-empty set
    /// of event triggers (the generator is not vacuously always empty).
    #[test]
    fn event_triggers_are_actually_generated() {
        let pool = vec![id("app")];
        let strategy = arb_event_triggers(pool);
        let mut runner = TestRunner::default();
        let mut saw_non_empty = false;
        for _ in 0..256 {
            let (_funcs, ets) = strategy.new_tree(&mut runner).unwrap().current();
            if !ets.is_empty() {
                saw_non_empty = true;
                break;
            }
        }
        assert!(
            saw_non_empty,
            "expected at least one non-empty event-trigger set across 256 draws",
        );
    }

    /// `table_rewrite` event triggers may only filter on rewrite-capable tags.
    /// PG rejects `CREATE TABLE` / `DROP TABLE` for `table_rewrite` at
    /// `CREATE EVENT TRIGGER` time, so the generator must never emit them.
    #[test]
    fn table_rewrite_only_uses_valid_tags() {
        use pgevolve_core::ir::event_trigger::EventTriggerEvent;

        let pool = vec![id("app"), id("public")];
        let strategy = arb_event_triggers(pool);
        let mut runner = TestRunner::default();
        for _ in 0..512 {
            let (_funcs, ets) = strategy.new_tree(&mut runner).unwrap().current();
            for et in &ets {
                if et.event == EventTriggerEvent::TableRewrite {
                    for tag in &et.tag_filter {
                        assert_eq!(
                            tag,
                            "ALTER TABLE",
                            "table_rewrite event trigger '{}' has invalid tag '{tag}'",
                            et.name.as_str(),
                        );
                    }
                }
            }
        }
    }

    /// An empty schema pool yields no event triggers (nowhere to host a
    /// function).
    #[test]
    fn empty_schema_pool_yields_nothing() {
        let strategy = arb_event_triggers(Vec::new());
        let mut runner = TestRunner::default();
        for _ in 0..32 {
            let (funcs, ets) = strategy.new_tree(&mut runner).unwrap().current();
            assert!(funcs.is_empty());
            assert!(ets.is_empty());
        }
    }
}
