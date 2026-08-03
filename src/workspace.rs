use crate::{SyntaxKind, parse};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const DEFAULT_LAYER_PRIORITY: i32 = 0;

/// Describes the BitBake search scope that produced a workspace candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceSearchScope {
    /// An absolute path or a file beside the referencing metadata file.
    CurrentFile,
    /// A file found through the effective BBPATH search path.
    Bbpath,
    /// A class found in a classes-recipe directory on BBPATH.
    ClassesRecipe,
    /// A class found in a classes directory on BBPATH.
    Classes,
}

/// Identifies the directive semantics used for a metadata file lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceFileDirective {
    /// Search beside the referencing file and then select the first BBPATH match.
    Include,
    /// Search only BBPATH and retain every matching file.
    IncludeAll,
    /// Search beside the referencing file and then retain the BBPATH matches.
    Require,
}

/// A candidate returned by BitBake-aware workspace resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceCandidate<'a> {
    path: &'a Path,
    layer: &'a Path,
    priority: i32,
    collection: Option<&'a str>,
    scope: WorkspaceSearchScope,
}

impl<'a> WorkspaceCandidate<'a> {
    /// Returns the resolved metadata path.
    pub fn path(self) -> &'a Path {
        self.path
    }

    /// Returns the layer root containing the candidate.
    pub fn layer(self) -> &'a Path {
        self.layer
    }

    /// Returns the BBFILE_PRIORITY used to rank the candidate's layer.
    pub const fn priority(self) -> i32 {
        self.priority
    }

    /// Returns the collection selected by the layer's BBFILE_PATTERN metadata.
    pub fn collection(self) -> Option<&'a str> {
        self.collection
    }

    /// Returns the BitBake search scope that produced the candidate.
    pub const fn scope(self) -> WorkspaceSearchScope {
        self.scope
    }
}

/// A conservative index of the complete BitBake layers represented by a set
/// of input paths.
///
/// The index deliberately only considers files supplied by the caller. This
/// makes single-file linting safe: semantic findings are emitted only when a
/// complete layer, including its conf/layer.conf, is present in the input
/// set. Layer metadata is used to build a deterministic BBPATH search order;
/// layer priority remains the fallback when the supplied metadata does not
/// describe a more specific path order.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceIndex {
    layers: Vec<LayerIndex>,
    search_paths: Vec<SearchPath>,
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

        let mut index = Self {
            layers,
            search_paths: Vec::new(),
        };
        index.search_paths = build_search_paths(&index.layers);
        Ok(index)
    }

    /// Returns whether path belongs to a complete indexed layer.
    pub fn is_complete_for(&self, path: &Path) -> bool {
        let path = canonicalize_for_lookup(path);
        self.layers
            .iter()
            .any(|layer| layer.is_complete() && layer.files.contains(&path))
    }

    /// Returns all matching static class definitions in BitBake search order.
    ///
    /// For inherit, BitBake searches all classes-recipe directories on
    /// BBPATH before searching the ordinary classes directories. The returned
    /// candidates retain layer, collection, priority, and scope information so
    /// callers can explain or validate the resolution.
    pub fn class_candidates(&self, class: &str) -> Vec<WorkspaceCandidate<'_>> {
        if class.is_empty() || class.contains(['/', '\\']) {
            return Vec::new();
        }

        let mut candidates = Vec::new();
        for (directory, scope) in [
            ("classes-recipe", WorkspaceSearchScope::ClassesRecipe),
            ("classes", WorkspaceSearchScope::Classes),
        ] {
            for search_path in &self.search_paths {
                let layer = &self.layers[search_path.layer_index];
                let candidate = search_path
                    .path
                    .join(directory)
                    .join(format!("{class}.bbclass"));
                if let Some(path) = layer.find_exact(&candidate) {
                    append_candidate(&mut candidates, layer.make_candidate(path, scope));
                }
            }
        }
        candidates
    }

    /// Resolves a static class name against the indexed layer class files.
    ///
    /// The first candidate follows the documented BitBake class search order.
    /// Call class_candidates when callers need to inspect every possible
    /// definition.
    pub fn resolve_class(&self, class: &str) -> Option<&Path> {
        self.class_candidates(class)
            .into_iter()
            .next()
            .map(|candidate| candidate.path)
    }

    /// Returns all matching static metadata files for a directive.
    ///
    /// Include and require first check the directory containing from, then
    /// search BBPATH. IncludeAll skips the current directory and returns every
    /// BBPATH match in search order. The candidate list is intentionally not
    /// truncated for include or require, allowing lint callers to report
    /// same-priority ambiguity while resolve_file retains the first-match API.
    pub fn file_candidates_for<'a>(
        &'a self,
        from: &Path,
        target: &str,
        directive: WorkspaceFileDirective,
    ) -> Vec<WorkspaceCandidate<'a>> {
        let target_path = Path::new(target);
        if target_path.is_absolute() {
            return self
                .find_path(target_path)
                .map(|(index, path)| {
                    vec![self.layers[index].make_candidate(path, WorkspaceSearchScope::CurrentFile)]
                })
                .unwrap_or_default();
        }

        let from = canonicalize_for_lookup(from);
        let mut candidates = Vec::new();

        if !matches!(directive, WorkspaceFileDirective::IncludeAll)
            && let Some(parent) = from.parent()
            && let Some((index, path)) = self.find_path(&parent.join(target_path))
        {
            append_candidate(
                &mut candidates,
                self.layers[index].make_candidate(path, WorkspaceSearchScope::CurrentFile),
            );
            return candidates;
        }

        for search_path in &self.search_paths {
            let layer = &self.layers[search_path.layer_index];
            if let Some(path) = layer.find_exact(&search_path.path.join(target_path)) {
                append_candidate(
                    &mut candidates,
                    layer.make_candidate(path, WorkspaceSearchScope::Bbpath),
                );
            }
        }
        candidates
    }

    /// Returns all matching static metadata files using require semantics.
    pub fn file_candidates<'a>(&'a self, from: &Path, target: &str) -> Vec<WorkspaceCandidate<'a>> {
        self.file_candidates_for(from, target, WorkspaceFileDirective::Require)
    }

    /// Resolves a static metadata file reference using require semantics.
    ///
    /// Relative references are checked beside the referencing file first and
    /// then through the effective BBPATH. The first candidate is returned;
    /// call file_candidates_for when ambiguity information is required.
    pub fn resolve_file<'a>(&'a self, from: &Path, target: &str) -> Option<&'a Path> {
        self.file_candidates(from, target)
            .into_iter()
            .next()
            .map(|candidate| candidate.path)
    }

    /// Returns every file that include_all would parse.
    pub fn include_all_candidates<'a>(
        &'a self,
        from: &Path,
        target: &str,
    ) -> Vec<WorkspaceCandidate<'a>> {
        self.file_candidates_for(from, target, WorkspaceFileDirective::IncludeAll)
    }

    fn find_path<'a>(&'a self, candidate: &Path) -> Option<(usize, &'a Path)> {
        if let Some(found) = self
            .layers
            .iter()
            .enumerate()
            .find_map(|(index, layer)| layer.find_exact(candidate).map(|path| (index, path)))
        {
            return Some(found);
        }

        if !needs_canonicalization(candidate) {
            return None;
        }
        let canonical = fs::canonicalize(candidate).ok()?;
        self.layers
            .iter()
            .enumerate()
            .find_map(|(index, layer)| layer.find_exact(&canonical).map(|path| (index, path)))
    }
}

#[derive(Clone, Debug)]
struct LayerIndex {
    root: PathBuf,
    priority: i32,
    metadata: LayerMetadata,
    files: BTreeSet<PathBuf>,
}

impl LayerIndex {
    fn new(root: PathBuf, all_files: &BTreeSet<PathBuf>) -> Self {
        let files = all_files
            .iter()
            .filter(|path| path.starts_with(&root))
            .cloned()
            .collect::<BTreeSet<_>>();
        let metadata = parse_layer_metadata(&root, &files);
        let priority = metadata.priority();

        Self {
            root,
            priority,
            metadata,
            files,
        }
    }

    fn is_complete(&self) -> bool {
        self.files.contains(&self.root.join("conf/layer.conf"))
    }

    fn find_exact<'a>(&'a self, candidate: &Path) -> Option<&'a Path> {
        self.files.get(candidate).map(PathBuf::as_path)
    }

    fn make_candidate<'a>(
        &'a self,
        path: &'a Path,
        scope: WorkspaceSearchScope,
    ) -> WorkspaceCandidate<'a> {
        WorkspaceCandidate {
            path,
            layer: &self.root,
            priority: self.priority,
            collection: self.metadata.collection_for(path, &self.root),
            scope,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct LayerMetadata {
    collections: Vec<String>,
    patterns: BTreeMap<String, String>,
    priorities: BTreeMap<String, i32>,
    bbpath: Vec<PathBuf>,
}

impl LayerMetadata {
    fn priority(&self) -> i32 {
        self.collections
            .iter()
            .filter_map(|collection| self.priorities.get(collection))
            .copied()
            .max()
            .or_else(|| self.priorities.values().copied().max())
            .unwrap_or(DEFAULT_LAYER_PRIORITY)
    }

    fn collection_for<'a>(&'a self, path: &Path, root: &Path) -> Option<&'a str> {
        self.collections
            .iter()
            .find(|collection| {
                self.patterns
                    .get(*collection)
                    .is_some_and(|pattern| pattern_matches_path(pattern, path, root))
            })
            .map(String::as_str)
            .or_else(|| (self.collections.len() == 1).then(|| self.collections[0].as_str()))
    }
}

#[derive(Clone, Debug)]
struct SearchPath {
    path: PathBuf,
    layer_index: usize,
}

fn build_search_paths(layers: &[LayerIndex]) -> Vec<SearchPath> {
    let mut search_paths = Vec::new();
    for (layer_index, layer) in layers.iter().enumerate() {
        let paths = if layer.metadata.bbpath.is_empty() {
            vec![layer.root.clone()]
        } else {
            layer.metadata.bbpath.clone()
        };

        for path in paths {
            let path = canonicalize_for_lookup(&path);
            let owner = layers
                .iter()
                .position(|candidate| path.starts_with(&candidate.root))
                .unwrap_or(layer_index);
            if search_paths
                .iter()
                .any(|entry: &SearchPath| entry.path == path)
            {
                continue;
            }
            search_paths.push(SearchPath {
                path,
                layer_index: owner,
            });
        }
    }
    search_paths
}

fn append_candidate<'a>(
    candidates: &mut Vec<WorkspaceCandidate<'a>>,
    candidate: WorkspaceCandidate<'a>,
) {
    if !candidates
        .iter()
        .any(|existing| existing.path == candidate.path)
    {
        candidates.push(candidate);
    }
}

fn parse_layer_metadata(root: &Path, files: &BTreeSet<PathBuf>) -> LayerMetadata {
    let layer_configuration = root.join("conf/layer.conf");
    if !files.contains(&layer_configuration) {
        return LayerMetadata::default();
    }

    let Ok(source) = fs::read_to_string(layer_configuration) else {
        return LayerMetadata::default();
    };
    let Ok(tree) = parse(&source) else {
        return LayerMetadata::default();
    };

    let mut metadata = LayerMetadata::default();
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        let base_name = assignment
            .name()
            .split(':')
            .next()
            .unwrap_or(assignment.name());
        match base_name {
            "BBPATH" => {
                for path in parse_bbpath(assignment.value(), root) {
                    if !metadata.bbpath.contains(&path) {
                        metadata.bbpath.push(path);
                    }
                }
            }
            "BBFILE_COLLECTIONS" => {
                for collection in static_words(assignment.value()) {
                    if !metadata.collections.contains(&collection) {
                        metadata.collections.push(collection);
                    }
                }
            }
            name if name.starts_with("BBFILE_PATTERN_") => {
                if let Some(collection) = name.strip_prefix("BBFILE_PATTERN_")
                    && let Some(pattern) = scalar_value(assignment.value())
                {
                    metadata.patterns.insert(collection.to_owned(), pattern);
                }
            }
            name if name.starts_with("BBFILE_PRIORITY_") => {
                if let Some(collection) = name.strip_prefix("BBFILE_PRIORITY_")
                    && let Some(priority) = parse_integer_value(assignment.value())
                {
                    metadata.priorities.insert(collection.to_owned(), priority);
                }
            }
            _ => {}
        }
    }
    metadata
}

fn parse_bbpath(value: &str, root: &Path) -> Vec<PathBuf> {
    let value = scalar_value(value).unwrap_or_default();
    let value = value.replace("\\\r\n", "").replace("\\\n", "");
    value
        .split(':')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let expanded = entry
                .replace("${LAYERDIR}", &root.to_string_lossy())
                .replace("${THISDIR}", &root.join("conf").to_string_lossy());
            let path = PathBuf::from(expanded);
            Some(if path.is_absolute() {
                path
            } else {
                root.join(path)
            })
        })
        .collect()
}

fn static_words(value: &str) -> Vec<String> {
    scalar_value(value)
        .unwrap_or_default()
        .split_ascii_whitespace()
        .filter(|word| !word.contains("${") && !word.contains("${@"))
        .map(str::to_owned)
        .collect()
}

fn scalar_value(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(quote) = value.as_bytes().first().copied()
        && matches!(quote, b'\'' | b'"')
    {
        let end = value[1..].find(quote as char)? + 1;
        return Some(value[1..end].to_owned());
    }
    Some(value.split('#').next()?.trim().to_owned())
}

fn parse_integer_value(value: &str) -> Option<i32> {
    scalar_value(value)?.parse().ok()
}

fn pattern_matches_path(pattern: &str, path: &Path, root: &Path) -> bool {
    let expanded = pattern
        .trim()
        .trim_start_matches('^')
        .replace("${LAYERDIR}", &root.to_string_lossy());
    let prefix = expanded
        .split(['*', '?', '[', '(', '$'])
        .next()
        .unwrap_or(&expanded)
        .trim_end_matches('/');
    !prefix.is_empty() && path.to_string_lossy().starts_with(prefix)
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

fn needs_canonicalization(path: &Path) -> bool {
    !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
}
