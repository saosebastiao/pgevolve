//! `pgevolve cluster init` — scaffold a new cluster project.
//!
//! Creates:
//! - `<path>/pgevolve-cluster.toml` with placeholder values (skipped if
//!   already present).
//! - `<path>/roles/` empty directory.
//! - Extends or creates `<path>/.gitignore` with a `cluster-plans/` entry.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Run `pgevolve cluster init`.
pub fn run(path: Option<PathBuf>) -> Result<i32> {
    let root = path.unwrap_or_else(|| PathBuf::from("."));

    // roles/ directory.
    std::fs::create_dir_all(root.join("roles"))
        .with_context(|| format!("creating {}", root.join("roles").display()))?;

    // pgevolve-cluster.toml — only write if absent.
    let toml_path = root.join("pgevolve-cluster.toml");
    if toml_path.exists() {
        eprintln!("Skipped existing {}", toml_path.display());
    } else {
        std::fs::write(&toml_path, DEFAULT_CONFIG)
            .with_context(|| format!("writing {}", toml_path.display()))?;
        eprintln!("Wrote {}", toml_path.display());
    }

    // .gitignore — extend if present, create if not.
    append_gitignore(&root.join(".gitignore"))
        .with_context(|| format!("updating {}", root.join(".gitignore").display()))?;

    println!("Initialized pgevolve cluster project at {}", root.display());
    Ok(0)
}

const DEFAULT_CONFIG: &str = r#"# pgevolve cluster project config — see https://github.com/saosebastiao/pgevolve

[project]
name = "my-cluster"

[connection]
# Superuser DSN. pgevolve needs SELECT on pg_authid.
dsn = "postgresql://postgres@localhost:5432/postgres"

# [bootstrap]
# roles = ["postgres", "cloudsqlsuperuser"]
"#;

fn append_gitignore(path: &Path) -> Result<()> {
    const MARKER: &str = "# pgevolve cluster";
    let entry = "# pgevolve cluster\ncluster-plans/\n";

    let existing = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    if existing.contains(MARKER) {
        return Ok(());
    }

    let mut new = existing;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(entry);
    std::fs::write(path, new).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_project_structure() {
        let dir = tempfile::tempdir().unwrap();
        let code = run(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(code, 0);
        assert!(dir.path().join("pgevolve-cluster.toml").exists());
        assert!(dir.path().join("roles").is_dir());
        assert!(dir.path().join(".gitignore").exists());
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("cluster-plans/"));
    }

    #[test]
    fn skips_existing_toml() {
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("pgevolve-cluster.toml");
        std::fs::write(&toml, "original").unwrap();
        run(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(std::fs::read_to_string(&toml).unwrap(), "original");
    }

    #[test]
    fn gitignore_not_duplicated_on_rerun() {
        let dir = tempfile::tempdir().unwrap();
        run(Some(dir.path().to_path_buf())).unwrap();
        run(Some(dir.path().to_path_buf())).unwrap();
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(gitignore.matches("cluster-plans/").count(), 1);
    }

    #[test]
    fn extends_existing_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        run(Some(dir.path().to_path_buf())).unwrap();
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("target/"));
        assert!(gitignore.contains("cluster-plans/"));
    }
}
