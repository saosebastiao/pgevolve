//! The three entry points pgevolve uses: parse, deparse, and the plpgsql analyzer.
//!
//! Each has the same shape. Marshal a Rust string into a `CString`, hand the
//! pointer to C, check the returned `error` pointer *before* touching any other
//! field of the result, convert what came back, and free the result — on both the
//! success and failure paths, which is why every function here converts into a
//! local before freeing rather than returning early.

// This is an FFI crate; calling into C is the entire purpose. The workspace
// denies `unsafe_code` so that pgevolve's own logic stays unsafe-free, and this
// module is the one place where that cannot hold. Every block below is a call
// into `libpg_query` or a read of a pointer it just returned, and each is
// immediately followed by the matching `pg_query_free_*`.
#![allow(
    unsafe_code,
    reason = "FFI boundary: this module exists to call libpg_query's C API"
)]

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use prost::Message;

use crate::error::{Error, Result};
use crate::ffi;
use crate::parse_result::ParseResult;
use crate::protobuf;

/// Read a C string the library just returned, without taking ownership.
///
/// `to_string_lossy` rather than a UTF-8 error: Postgres error messages can carry
/// bytes from the offending input, and a parse failure reported as an encoding
/// failure would be actively misleading.
///
/// # Safety
///
/// `ptr` must be non-null and point at a NUL-terminated string owned by the
/// library, valid until the matching `pg_query_free_*` call.
unsafe fn owned_string(ptr: *const c_char) -> String {
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Parse SQL into Postgres's own parse tree.
///
/// Accepts multiple statements; each becomes one entry in
/// `ParseResult::protobuf.stmts`.
///
/// # Example
///
/// ```
/// let result = pgevolve_pgquery::parse("SELECT 1; SELECT 2;").expect("parses");
/// assert_eq!(result.protobuf.stmts.len(), 2);
/// ```
pub fn parse(statement: &str) -> Result<ParseResult> {
    let input = CString::new(statement)?;
    let result = unsafe { ffi::pg_query_parse_protobuf(input.as_ptr()) };

    let parsed = if result.error.is_null() {
        // Safe only because `error` was null: on the error path these fields are
        // not guaranteed to be initialized.
        let data = unsafe {
            std::slice::from_raw_parts(
                result.parse_tree.data.cast::<u8>().cast_const(),
                result.parse_tree.len,
            )
        };
        let stderr = unsafe { owned_string(result.stderr_buffer) };
        protobuf::ParseResult::decode(data)
            .map_err(Error::Decode)
            .map(|protobuf| ParseResult::new(protobuf, &stderr))
    } else {
        Err(Error::Parse(unsafe {
            owned_string((*result.error).message)
        }))
    };

    unsafe { ffi::pg_query_free_protobuf_parse_result(result) };
    parsed
}

/// Render a parse tree back into SQL.
///
/// The deparser is Postgres's own, so the output is canonical: two spellings of
/// the same statement converge on one byte string. pgevolve relies on that for
/// equality, which is why a deparse failure is an error here rather than a
/// fallback to the input text.
///
/// # Example
///
/// ```
/// let result = pgevolve_pgquery::parse("select  1").expect("parses");
/// assert_eq!(
///     pgevolve_pgquery::deparse(&result.protobuf).expect("deparses"),
///     "SELECT 1"
/// );
/// ```
pub fn deparse(protobuf: &protobuf::ParseResult) -> Result<String> {
    let buffer = protobuf.encode_to_vec();

    // `PgQueryProtobuf.data` is declared `*mut c_char` although the deparser
    // only reads it. Casting away const is required by the C signature; the
    // buffer stays alive and unaliased for the duration of the call.
    let request = ffi::PgQueryProtobuf {
        data: buffer.as_ptr().cast::<c_char>().cast_mut(),
        len: buffer.len(),
    };
    let result = unsafe { ffi::pg_query_deparse_protobuf(request) };

    let deparsed = if result.error.is_null() {
        Ok(unsafe { owned_string(result.query) })
    } else {
        Err(Error::Parse(unsafe {
            owned_string((*result.error).message)
        }))
    };

    unsafe { ffi::pg_query_free_deparse_result(result) };
    // Named so the buffer's lifetime is visibly tied to the call above rather
    // than to wherever the optimiser would otherwise be free to drop it.
    drop(buffer);
    deparsed
}

/// Run Postgres's plpgsql analyzer over a `CREATE FUNCTION` statement.
///
/// Returns the analyzer's raw JSON. pgevolve depends on the analyzer's
/// *semantics*, not just its syntax check: it picks a `SETOF` versus `void`
/// wrapper based on whether the analyzer rejects `RETURN QUERY`, so a change in
/// what this accepts is a change in what pgevolve generates.
///
/// # Example
///
/// ```
/// let json = pgevolve_pgquery::parse_plpgsql(
///     "CREATE FUNCTION f() RETURNS void AS $$ BEGIN RETURN; END; $$ LANGUAGE plpgsql;"
/// ).expect("analyzes");
/// assert!(json.is_array());
/// ```
pub fn parse_plpgsql(statement: &str) -> Result<serde_json::Value> {
    let input = CString::new(statement)?;
    let result = unsafe { ffi::pg_query_parse_plpgsql(input.as_ptr()) };

    let analyzed = if result.error.is_null() {
        let raw = unsafe { owned_string(result.plpgsql_funcs) };
        serde_json::from_str(&raw).map_err(|e| Error::InvalidJson(e.to_string()))
    } else {
        Err(Error::Parse(unsafe {
            owned_string((*result.error).message)
        }))
    };

    unsafe { ffi::pg_query_free_plpgsql_parse_result(result) };
    analyzed
}

#[cfg(test)]
mod tests {
    use super::{deparse, parse, parse_plpgsql};

    #[test]
    fn deparse_canonicalizes_spelling() {
        // Two spellings, one byte string — the property pgevolve's equality
        // checks are built on.
        let a = parse("select 1 from  t").expect("parses");
        let b = parse("SELECT   1 FROM t").expect("parses");
        assert_eq!(
            deparse(&a.protobuf).expect("deparses"),
            deparse(&b.protobuf).expect("deparses")
        );
    }

    #[test]
    fn interior_nul_is_rejected_before_reaching_c() {
        let err = parse("SELECT\0 1").expect_err("interior NUL is rejected");
        assert!(matches!(err, super::Error::Conversion(_)), "got {err:?}");
    }

    #[test]
    fn plpgsql_analyzer_rejects_return_query_without_setof() {
        // This is the behaviour pgevolve's SETOF-vs-void wrapper choice depends
        // on. If it ever stops erroring, that decision silently changes.
        let result = parse_plpgsql(
            "CREATE FUNCTION f() RETURNS integer AS $$ BEGIN \
             RETURN QUERY SELECT 1; END; $$ LANGUAGE plpgsql;",
        );
        assert!(
            result.is_err(),
            "analyzer accepted RETURN QUERY in a non-SETOF function: {result:?}"
        );
    }

    #[test]
    fn plpgsql_analyzer_accepts_a_valid_body() {
        let json = parse_plpgsql(
            "CREATE FUNCTION f() RETURNS void AS $$ BEGIN RETURN; END; $$ LANGUAGE plpgsql;",
        )
        .expect("analyzes");
        assert!(json.is_array(), "expected a JSON array, got {json}");
    }

    #[test]
    fn deparse_error_surfaces_rather_than_panicking() {
        // An empty parse result has nothing to deparse; it must not crash.
        let empty = crate::protobuf::ParseResult {
            version: crate::vendored_pg_version_num(),
            stmts: Vec::new(),
        };
        assert_eq!(deparse(&empty).expect("empty deparses"), "");
    }
}
