//! `pgevolve init` — scaffold a new project.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::cli::InitArgs;

/// Run `pgevolve init`.
pub fn run(args: InitArgs) -> Result<i32> {
    let dir = args.dir.unwrap_or_else(|| PathBuf::from("."));
    let cfg_path = dir.join("pgevolve.toml");
    if cfg_path.exists() && !args.force {
        return Err(anyhow!(
            "pgevolve.toml already exists at {} (pass --force to overwrite)",
            cfg_path.display(),
        ));
    }
    std::fs::create_dir_all(dir.join("schema"))
        .with_context(|| format!("creating {}", dir.join("schema").display()))?;
    std::fs::create_dir_all(dir.join("plans"))
        .with_context(|| format!("creating {}", dir.join("plans").display()))?;
    std::fs::write(&cfg_path, DEFAULT_CONFIG)
        .with_context(|| format!("writing {}", cfg_path.display()))?;
    append_gitignore(&dir.join(".gitignore"))?;
    println!("Initialized pgevolve project at {}", dir.display());
    Ok(0)
}

const DEFAULT_CONFIG: &str = include_str!("../../templates/pgevolve.toml");

fn append_gitignore(path: &Path) -> Result<()> {
    let entries = "\n# pgevolve\n# applied plan directories are not generated artifacts;\n# keep them under version control. Uncomment to ignore instead.\n# plans/\n";
    let existing = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };
    if existing.contains("# pgevolve") {
        return Ok(());
    }
    let mut new = existing;
    new.push_str(entries);
    std::fs::write(path, new).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_project_structure() {
        let dir = tempfile::tempdir().unwrap();
        let code = run(InitArgs {
            dir: Some(dir.path().into()),
            force: false,
        })
        .unwrap();
        assert_eq!(code, 0);
        assert!(dir.path().join("pgevolve.toml").exists());
        assert!(dir.path().join("schema").is_dir());
        assert!(dir.path().join("plans").is_dir());
        assert!(dir.path().join(".gitignore").exists());
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        run(InitArgs {
            dir: Some(dir.path().into()),
            force: false,
        })
        .unwrap();
        let err = run(InitArgs {
            dir: Some(dir.path().into()),
            force: false,
        })
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn init_force_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        run(InitArgs {
            dir: Some(dir.path().into()),
            force: false,
        })
        .unwrap();
        let code = run(InitArgs {
            dir: Some(dir.path().into()),
            force: true,
        })
        .unwrap();
        assert_eq!(code, 0);
    }
}
