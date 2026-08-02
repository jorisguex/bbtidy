use crate::{SyntaxKind, parse};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const DEFAULT_LAYER_PRIORITY: i32 = 0;

/// A candidate returned by priority-aware workspace resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceCandidate<'a> {
    path: &'a Path,
    priority: i32,
}

impl<'a> WorkspaceCandidate<'a> {
    /// Returns the resolved metadata path.
    pub fn path(self) -> &'a Path {
        self.path
    }

    /// Returns the `BBFILE_PRIORITY` used to rank this candidate.
    pub const fn priority(self) -> i32 {
        self.priority
    }
}

/// A conservative index of the complete BitBake layers represented by a set
/// of input paths.
///
/// The index deliberately only considers files supplied by the caller. This
/// makes single-file linting safe: semantic findings are emitted only when a
/// complete layer, including its `conf/layer.conf`, is present in the input
/// set. Complete layers are ordered by their `BBFILE_PRIORITY_*` assignment;
/// paths are used as a deterministic tie-breaker when a layer omits an
/// explicit priority.
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

        let mut layers = roots
            .into_iter()
            .map(|root| LayerIndex::new(root, &files))
            .collect::<Vec<_>>();
        layers.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.root.cmp(&right.root))
        });
        Ok(Self { layers })
    }

    /// Returns whether `path` belongs to a complete indexed layer.
    pub fn is_complete_for(&self, path: &Path) -> bool {
        let path = canonicalize_for_lookup(path);
        self.layers
            .iter()
            .any(|layer| layer.is_complete() && layer.files.contains(&path))
    }

    /// Returns all matching static class definitions, ordered by layer
    /// priority and then by canonical path.
    pub fn class_candidates(&self, class: &str) -> Vec<WorkspaceCandidate<'_>> {
        let mut candidates = Vec::new();
        for layer in self.layers.iter().filter(|layer| layer.is_complete()) {
            if let Some(paths) = layer.classes.get(class) {
                candidates.extend(paths.iter().map(|path| WorkspaceCandidate {
                    path: path.as_path(),
                    priority: layer.priority,
                }));
            }
        }
        candidates
    }

    /// Resolves a static class name against the indexed layer class files.
    ///
    /// When multiple definitions exist, the highest-priority definition is
    /// returned. Call [`Self::class_candidates`] when callers need to detect
    /// same-priority ambiguity instead of accepting the selected definition.
    pub fn resolve_class(&self, class: &str) -> Option<&Path> {
        self.layers
            .iter()
            .filter(|layer| layer.is_complete())
            .find_map(|layer| {
                layer
                    .classes
                    .get(class)
                    .and_then(|paths| paths.first().map(PathBuf::as_path))
            })
    }

    /// Returns all matching static metadata files, ordered by BitBake-style
    /// search scope and then by layer priority.
    ///
    /// A file beside the referencing file takes precedence. If no local file
    /// matches, the indexed layers are searched by descending
    /// `BBFILE_PRIORITY_*`; same-priority candidates are retained so callers
    /// can report ambiguity rather than silently choosing one.
    pub fn file_candidates<'a>(&'a self, from: &Path, target: &str) -> Vec<WorkspaceCandidate<'a>> {
        let from = canonicalize_for_lookup(from);
        let originating_layer = self
            .layers
            .iter()
            .position(|layer| layer.is_complete() && layer.files.contains(&from));

        if let Some(index) = originating_layer {
            let local = self.layers[index].find_candidates(&from, target);
            if !local.is_empty() {
                return local
                    .into_iter()
                    .map(|path| WorkspaceCandidate {
                        path,
                        priority: self.layers[index].priority,
                    })
                    .collect();
            }
        }

        let mut candidates = Vec::new();
        for (index, layer) in self.layers.iter().enumerate() {
            if !layer.is_complete() || Some(index) == originating_layer {
                continue;
            }
            candidates.extend(
                layer
                    .find_candidates(&from, target)
                    .into_iter()
                    .map(|path| WorkspaceCandidate {
                        path,
                        priority: layer.priority,
                    }),
            );
        }
        candidates
    }

    /// Resolves a static metadata file reference.
    ///
    /// Relative references are checked beside the referencing file first and
    /// then from each indexed layer root. A priority-ranked filename match is
    /// used as a final convenience for BitBake's layer search behavior. Call
    /// [`Self::file_candidates`] when ambiguity information is required.
    pub fn resolve_file<'a>(&'a self, from: &Path, target: &str) -> Option<&'a Path> {
        self.file_candidates(from, target)
            .into_iter()
            .next()
            .map(|candidate| candidate.path)
    }
}

#[derive(Clone, Debug)]
struct LayerIndex {
    root: PathBuf,
    priority: i32,
    files: BTreeSet<PathBuf>,
    classes: BTreeMap<String, Vec<PathBuf>>,
    names: BTreeMap<String, BTreeSet<PathBuf>>,
}

impl LayerIndex {
    fn new(root: PathBuf, all_files: &BTreeSet<PathBuf>) -> Self {
        let files = all_files
            .iter()
            .filter(|path| path.starts_with(&root))
            .cloned()
            .collect::<BTreeSet<_>>();
        let priority = parse_layer_priority(&root, &files);
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
                    .or_insert_with(Vec::new)
                    .push(path.clone());
            }
        }

        Self {
            root,
            priority,
            files,
            classes,
            names,
        }
    }

    fn is_complete(&self) -> bool {
        self.files.contains(&self.root.join("conf/layer.conf"))
    }

    fn find_candidates<'a>(&'a self, from: &Path, target: &str) -> Vec<&'a Path> {
        let target_path = Path::new(target);
        let mut candidates = Vec::new();

        if target_path.is_absolute() {
            self.append_candidate(target_path, &mut candidates);
            return candidates;
        }

        if let Some(parent) = from.parent() {
            self.append_candidate(&parent.join(target_path), &mut candidates);
        }
        self.append_candidate(&self.root.join(target_path), &mut candidates);

        if target_path.components().count() == 1
            && let Some(matches) = self.names.get(target)
        {
            for path in matches {
                self.append_path(path.as_path(), &mut candidates);
            }
        }
        candidates
    }

    fn append_candidate<'a>(&'a self, candidate: &Path, candidates: &mut Vec<&'a Path>) {
        if let Some(path) = self.find_candidate(candidate) {
            self.append_path(path, candidates);
        }
    }

    fn append_path<'a>(&'a self, path: &'a Path, candidates: &mut Vec<&'a Path>) {
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }

    fn find_candidate<'a>(&'a self, candidate: &Path) -> Option<&'a Path> {
        if let Some(path) = self.files.get(candidate) {
            return Some(path.as_path());
        }
        let candidate = fs::canonicalize(candidate).ok()?;
        self.files.get(&candidate).map(PathBuf::as_path)
    }
}

fn parse_layer_priority(root: &Path, files: &BTreeSet<PathBuf>) -> i32 {
    let layer_configuration = root.join("conf/layer.conf");
    if !files.contains(&layer_configuration) {
        return DEFAULT_LAYER_PRIORITY;
    }

    let Ok(source) = fs::read_to_string(layer_configuration) else {
        return DEFAULT_LAYER_PRIORITY;
    };
    let Ok(tree) = parse(&source) else {
        return DEFAULT_LAYER_PRIORITY;
    };

    tree.nodes()
        .iter()
        .filter_map(|node| match node.kind() {
            SyntaxKind::Assignment(assignment) => Some(assignment),
            _ => None,
        })
        .filter(|assignment| assignment.name().starts_with("BBFILE_PRIORITY_"))
        .filter_map(|assignment| parse_integer_value(assignment.value()))
        .max()
        .unwrap_or(DEFAULT_LAYER_PRIORITY)
}

fn parse_integer_value(value: &str) -> Option<i32> {
    let value = value.trim();
    let value = if let Some(quote) = value.as_bytes().first().copied()
        && matches!(quote, b'\'' | b'"')
    {
        let end = value[1..].find(quote as char)? + 1;
        &value[1..end]
    } else {
        value.split('#').next()?.trim()
    };
    value.parse().ok()
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
