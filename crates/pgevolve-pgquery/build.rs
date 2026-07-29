//! Build the vendored `libpg_query` C library and generate its FFI declarations.
//!
//! Differences from upstream `pg_query.rs`'s `build.rs`, all deliberate:
//!
//! - **No protoc, ever.** Upstream regenerates `src/protobuf.rs` with `prost-build`
//!   whenever `protoc` happens to be on `PATH`, and it does so by pointing
//!   `OUT_DIR` at its own `src/` directory and renaming the output over the
//!   checked-in file. On a machine with `protoc` installed that mutates the Cargo
//!   registry cache — an immutable-by-contract directory. It is reproducible: in
//!   this repo's own container, `src/protobuf.rs` in the extracted 6.1.1 crate
//!   carries today's mtime while every sibling file carries the canonical
//!   registry timestamp. Here `protobuf.rs` is an ordinary checked-in source
//!   file and nothing regenerates it during a build.
//! - **No copy into `OUT_DIR`.** Upstream copies the whole vendored tree with
//!   `fs_extra` before compiling. `cc` writes its objects to `OUT_DIR` regardless,
//!   so the copy bought nothing and cost a tree walk of ~11 MB per build.
//! - **No Make.** The vendored `Makefile` is not shipped; this file is the build.
//! - **Explicit flags.** `-fno-strict-aliasing` and `-fwrapv` are not optional
//!   for Postgres sources: the C is written assuming both, and a compiler that
//!   assumes otherwise is free to miscompile overflow checks and aliased casts.
//!   Upstream relied on `cc`'s defaults.

use std::env;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let vendor = Path::new("libpg_query");

    // Rebuild when any vendored source changes, but not on every unrelated edit.
    println!("cargo:rerun-if-changed=libpg_query");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=pg_query");

    compile_c(vendor)?;
    generate_ffi(vendor, &out_dir)?;
    Ok(())
}

/// Compile `libpg_query` plus its two vendored dependencies into `libpg_query.a`.
fn compile_c(vendor: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut build = cc::Build::new();

    for pattern in ["src/*.c", "src/postgres/*.c"] {
        let full = vendor.join(pattern);
        let pattern = full.to_str().ok_or("non-UTF-8 vendor path")?;
        for entry in glob::glob(pattern)? {
            build.file(entry?);
        }
    }

    build
        .file(vendor.join("vendor/protobuf-c/protobuf-c.c"))
        .file(vendor.join("vendor/xxhash/xxhash.c"))
        .file(vendor.join("protobuf/pg_query.pb-c.c"))
        .include(vendor)
        .include(vendor.join("vendor"))
        .include(vendor.join("src/postgres/include"))
        .include(vendor.join("src/include"))
        // Postgres C assumes both of these; see the module comment.
        .flag_if_supported("-fno-strict-aliasing")
        .flag_if_supported("-fwrapv")
        // Upstream's warnings are upstream's business, and there are thousands.
        .warnings(false);

    // Upstream keys this off PROFILE/DEBUG. Assertion checking inside the
    // Postgres sources is genuinely useful when a *parser* bug is suspected, but
    // it is not free, and pgevolve's own debug builds are where the conformance
    // suite runs. Keep the same behaviour rather than silently changing it.
    if env::var("DEBUG").as_deref() == Ok("1") {
        build.define("USE_ASSERT_CHECKING", None);
    }

    let target = env::var("TARGET")?;
    if target.contains("windows") {
        build.include(vendor.join("src/postgres/include/port/win32"));
        if target.contains("msvc") {
            build.include(vendor.join("src/postgres/include/port/win32_msvc"));
        }
    }

    build.compile("pg_query");
    Ok(())
}

/// Generate Rust declarations for the handful of `pg_query.h` entry points.
///
/// Only the parse, deparse, and plpgsql families are used; the header is small
/// enough that generating all of it and letting dead-code elimination sort it out
/// is cheaper than maintaining an allowlist.
fn generate_ffi(vendor: &Path, out_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let header = vendor.join("pg_query.h");
    bindgen::Builder::default()
        .header(header.to_str().ok_or("non-UTF-8 header path")?)
        // Without this, a header edit does not retrigger generation.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .map_err(|e| format!("bindgen failed on pg_query.h: {e}"))?
        .write_to_file(out_dir.join("ffi.rs"))?;
    Ok(())
}
