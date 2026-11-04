//! Asset paths: `models/crate.glb#Mesh0` = a source file relative to the asset root, plus an
//! optional label naming one sub-asset inside it. Always forward slashes, case preserved.

use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetPath {
    source: String,
    label: Option<String>,
}

impl AssetPath {
    /// Parses `dir/file.ext#Label`. Backslashes become slashes; `.` segments are dropped.
    pub fn parse(s: &str) -> Self {
        let (source, label) = match s.split_once('#') {
            Some((a, b)) if !b.is_empty() => (a, Some(b.to_owned())),
            Some((a, _)) => (a, None),
            None => (s, None),
        };
        let source = source
            .replace('\\', "/")
            .split('/')
            .filter(|seg| !seg.is_empty() && *seg != ".")
            .collect::<Vec<_>>()
            .join("/");
        Self { source, label }
    }

    /// The path of `file` relative to `root`, if it is inside it.
    pub fn from_file(root: &Path, file: &Path) -> Option<Self> {
        let rel = file.strip_prefix(root).ok()?;
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        (!parts.is_empty()).then(|| Self {
            source: parts.join("/"),
            label: None,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// The same source without a label.
    pub fn source_path(&self) -> Self {
        Self {
            source: self.source.clone(),
            label: None,
        }
    }

    pub fn with_label(&self, label: impl Into<String>) -> Self {
        Self {
            source: self.source.clone(),
            label: Some(label.into()),
        }
    }

    /// Lower-case extension of the source (`"glb"`).
    pub fn extension(&self) -> String {
        Path::new(&self.source)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// File name without directories (`"crate.glb"`).
    pub fn file_name(&self) -> &str {
        self.source.rsplit('/').next().unwrap_or(&self.source)
    }

    pub fn to_file(&self, root: &Path) -> PathBuf {
        let mut p = root.to_path_buf();
        for seg in self.source.split('/') {
            p.push(seg);
        }
        p
    }
}

impl fmt::Display for AssetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.label {
            Some(l) => write!(f, "{}#{l}", self.source),
            None => f.write_str(&self.source),
        }
    }
}

impl From<&str> for AssetPath {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normalizes_and_splits_labels() {
        let p = AssetPath::parse(r".\models\crate.glb#Mesh0");
        assert_eq!(p.source(), "models/crate.glb");
        assert_eq!(p.label(), Some("Mesh0"));
        assert_eq!(p.to_string(), "models/crate.glb#Mesh0");
        assert_eq!(p.extension(), "glb");
        assert_eq!(p.file_name(), "crate.glb");
        assert_eq!(AssetPath::parse("a/b.png#").label(), None);
    }

    #[test]
    fn file_round_trip() {
        let root = Path::new("C:/proj/assets");
        let file = root.join("models").join("crate.glb");
        let p = AssetPath::from_file(root, &file).unwrap();
        assert_eq!(p.source(), "models/crate.glb");
        assert_eq!(p.to_file(root), file);
        assert!(AssetPath::from_file(root, Path::new("C:/elsewhere/x.png")).is_none());
    }
}
