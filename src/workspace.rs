use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A conservative index of the complete BitBake layers represented by a set
/// of input paths.
///
/// The index deliberately only considers files supplied by the caller. This
/// makes single-file linting safe: semantic findings are emitted only when a
/// complete layer, including its `conf/layer.conf`, is present in the input
/// set.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceIndex {
    layers: Vec<LayerIndex>,
}

impl WorkspaceIndex {
    /// Builds an index from existing metadata paths.
    ///
    /// Paths are canonicalized before indexing so symlinked workspaces and
    /// platform-specific path aliases resolve consistently. Missing paths are
    /// returned as I/O errors rather than silently producing an incomplete
    /// semantic context.
    pub fn from_paths<I, P>(paths: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let files = paths
            .into_iter()
            .map(|path| fs::canonicalize(path.as_ref()))
            .collect::<io::Result<BTreeSet<_>>>()?;
        let roots = files
            .iter()
            .filter_map(|path| supplied_layer_root_for(path))
            .collect::<BTreeSet<_>>();

        let layers = roots
            .into_iter()
            .map(|root| LayerIndex::new(root, &files))
            .collect();
        Ok(Self { layers })
    }

    /// Returns whether `path` belongs to a complete indexed layer.
    pub fn is_complete_for(&self, path: &Path) -> bool {
        let path = canonicalize_for_lookup(path);
        self.layers
            .iter()
            .any(|layer| layer.is_complete() && layer.files.contains(&path))
    }

    /// Resolves a static class name against the indexed layer class files.
    pub fn resolve_class(&self, class: &str) -> Option<&Path> {
        self.layers
            .iter()
            .filter(|layer| layer.is_complete())
            .find_map(|layer| layer.classes.get(class).map(PathBuf::as_path))
    }

    /// Resolves a static metadata file reference.
    ///
    /// Relative references are checked beside the referencing file first and
    /// then from each indexed layer root. A unique filename match is used as
    /// a final convenience for BitBake's layer search behavior.
    pub fn resolve_file<'a>(&'a self, from: &Path, target: &str) -> Option<&'a Path> {
        let from = canonicalize_for_lookup(from);
        let originating_layer = self
            .layers
            .iter()
            .position(|layer| layer.is_complete() && layer.files.contains(&from));

        if let Some(index) = originating_layer {
            if let Some(path) = self.layers[index].find_file(&from, target) {
                return Some(path);
            }
        }

        self.layers
            .iter()
            .enumerate()
            .filter(|(index, layer)| layer.is_complete() && Some(*index) != originating_layer)
            .find_map(|(_, layer)| layer.find_file(&from, target))
    }
}

#[derive(Clone, Debug)]
struct LayerIndex {
    root: PathBuf,
    files: BTreeSet<PathBuf>,
    classes: BTreeMap<String, PathBuf>,
    names: BTreeMap<String, BTreeSet<PathBuf>>,
}

impl LayerIndex {
    fn new(root: PathBuf, all_files: &BTreeSet<PathBuf>) -> Self {
        let files = all_files
            .iter()
            .filter(|path| path.starts_with(&root))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut classes = BTreeMap::new();
        let mut names = BTreeMap::new();

        for path in &files {
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            names
                .entry(file_name.to_owned())
                .or_insert_with(BTreeSet::new)
                .insert(path.clone());

            if path.extension().and_then(|extension| extension.to_str()) == Some("bbclass")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                classes
                    .entry(stem.to_owned())
                    .or_insert_with(|| path.clone());
            }
        }

        Self {
            root,
            files,
            classes,
            names,
        }
    }

    fn is_complete(&self) -> bool {
        self.files.contains(&self.root.join("conf/layer.conf"))
    }

    fn find_file<'a>(&'a self, from: &Path, target: &str) -> Option<&'a Path> {
        let target_path = Path::new(target);
        if target_path.is_absolute() {
            return self.find_candidate(target_path);
        }

        if let Some(parent) = from.parent()
            && let Some(path) = self.find_candidate(&parent.join(target_path))
        {
            return Some(path);
        }
        if let Some(path) = self.find_candidate(&self.root.join(target_path)) {
            return Some(path);
        }

        if target_path.components().count() == 1 {
            return self
                .names
                .get(target)
                .filter(|matches| matches.len() == 1)
                .and_then(|matches| matches.iter().next())
                .map(PathBuf::as_path);
        }
        None
    }

    fn find_candidate<'a>(&'a self, candidate: &Path) -> Option<&'a Path> {
        if let Some(path) = self.files.get(candidate) {
            return Some(path.as_path());
        }
        let candidate = fs::canonicalize(candidate).ok()?;
        self.files.get(&candidate).map(PathBuf::as_path)
    }
}

fn supplied_layer_root_for(path: &Path) -> Option<PathBuf> {
    if path.file_name().and_then(|name| name.to_str()) != Some("layer.conf") {
        return None;
    }
    let configuration_directory = path.parent()?;
    if configuration_directory
        .file_name()
        .and_then(|name| name.to_str())
        != Some("conf")
    {
        return None;
    }
    configuration_directory.parent().map(Path::to_path_buf)
}

fn canonicalize_for_lookup(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}
