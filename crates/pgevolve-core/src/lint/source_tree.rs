//! [`SourceTree`] — `Catalog` plus per-object source locations.
//!
//! Layout-aware lint rules need to know which file each object came from.
//! [`SourceTree`] couples the IR with that mapping.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::identifier::{Identifier, QualifiedName};
use crate::ir::catalog::Catalog;
use crate::parse::SourceLocation;

/// Identifier for one IR object in a [`SourceTree`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ObjectKey {
    /// A schema, identified by its name.
    Schema(Identifier),
    /// A table.
    Table(QualifiedName),
    /// An index.
    Index(QualifiedName),
    /// A sequence.
    Sequence(QualifiedName),
}

impl ObjectKey {
    /// Lowercase kind name (`schema` / `table` / `index` / `sequence`).
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Schema(_) => "schema",
            Self::Table(_) => "table",
            Self::Index(_) => "index",
            Self::Sequence(_) => "sequence",
        }
    }

    /// Plural form used by some layout profiles (`schemas` / `tables` / etc.).
    pub const fn kind_plural(&self) -> &'static str {
        match self {
            Self::Schema(_) => "schemas",
            Self::Table(_) => "tables",
            Self::Index(_) => "indexes",
            Self::Sequence(_) => "sequences",
        }
    }

    /// Schema name component:
    /// - `Schema(s)` — `s` itself
    /// - `Table(q) / Index(q) / Sequence(q)` — `q.schema`
    pub const fn schema(&self) -> &Identifier {
        match self {
            Self::Schema(s) => s,
            Self::Table(q) | Self::Index(q) | Self::Sequence(q) => &q.schema,
        }
    }

    /// Bare-name component:
    /// - `Schema(s)` — `s` itself
    /// - other variants — `q.name`
    pub const fn bare_name(&self) -> &Identifier {
        match self {
            Self::Schema(s) => s,
            Self::Table(q) | Self::Index(q) | Self::Sequence(q) => &q.name,
        }
    }
}

impl std::fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schema(s) => write!(f, "schema:{s}"),
            Self::Table(q) => write!(f, "table:{q}"),
            Self::Index(q) => write!(f, "index:{q}"),
            Self::Sequence(q) => write!(f, "sequence:{q}"),
        }
    }
}

/// `Catalog` plus a map from object identifiers to their source locations.
#[derive(Debug, Clone)]
pub struct SourceTree {
    /// IR catalog parsed from the source tree.
    pub catalog: Catalog,
    /// One entry per IR object: where in the source tree the object was
    /// declared.
    pub object_locations: HashMap<ObjectKey, SourceLocation>,
}

impl SourceTree {
    /// Construct from a parsed catalog and an object-keyed location map.
    pub const fn new(
        catalog: Catalog,
        object_locations: HashMap<ObjectKey, SourceLocation>,
    ) -> Self {
        Self {
            catalog,
            object_locations,
        }
    }

    /// Iterate all object keys.
    pub fn objects(&self) -> impl Iterator<Item = &ObjectKey> {
        self.object_locations.keys()
    }

    /// File path for `key`, if known.
    pub fn file_of(&self, key: &ObjectKey) -> Option<&Path> {
        self.object_locations.get(key).map(|l| l.file.as_path())
    }

    /// Group every object by the file path that declared it.
    pub fn objects_by_file(&self) -> HashMap<PathBuf, Vec<ObjectKey>> {
        let mut out: HashMap<PathBuf, Vec<ObjectKey>> = HashMap::new();
        for (k, loc) in &self.object_locations {
            out.entry(loc.file.clone()).or_default().push(k.clone());
        }
        for v in out.values_mut() {
            v.sort_by_key(ToString::to_string);
        }
        out
    }
}
