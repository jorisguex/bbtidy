use crate::{AssignmentOperator, DirectiveKeyword, SyntaxKind, SyntaxTree, comment_start, parse};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    /// A class found in a classes-global directory on BBPATH.
    ClassesGlobal,
    /// A class found in a classes directory on BBPATH.
    Classes,
}

impl WorkspaceSearchScope {
    /// Returns a stable human-readable name for this search scope.
    pub const fn description(self) -> &'static str {
        match self {
            Self::CurrentFile => "the current file directory",
            Self::Bbpath => "BBPATH",
            Self::ClassesRecipe => "classes-recipe on BBPATH",
            Self::ClassesGlobal => "classes-global on BBPATH",
            Self::Classes => "classes on BBPATH",
        }
    }
}

/// Selects the BitBake class namespace used while parsing metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WorkspaceClassContext {
    /// Configuration parsing and classes inherited through `INHERIT`.
    Global,
    /// Recipe parsing and classes inherited with the `inherit` directive.
    Recipe,
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

/// Identifies the directive that created a resolved static workspace edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum WorkspaceDependencyKind {
    Include,
    IncludeAll,
    Require,
    Inherit,
    InheritDefer,
    InheritGlobal,
    UserClasses,
}

impl WorkspaceDependencyKind {
    /// Returns the directive keyword represented by this dependency edge.
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::IncludeAll => "include_all",
            Self::Require => "require",
            Self::Inherit => "inherit",
            Self::InheritDefer => "inherit_defer",
            Self::InheritGlobal => "INHERIT",
            Self::UserClasses => "USER_CLASSES",
        }
    }
}

/// A resolved static dependency between two metadata files in an indexed workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceDependency<'a> {
    from: &'a Path,
    to: &'a Path,
    kind: WorkspaceDependencyKind,
}

impl<'a> WorkspaceDependency<'a> {
    /// Returns the metadata file containing the dependency directive.
    pub fn from(self) -> &'a Path {
        self.from
    }

    /// Returns the metadata file selected by BitBake search semantics.
    pub fn to(self) -> &'a Path {
        self.to
    }

    /// Returns the directive that introduced this dependency.
    pub const fn kind(self) -> WorkspaceDependencyKind {
        self.kind
    }
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

/// A conservative index of BitBake metadata and its static workspace context.
///
/// [`WorkspaceIndex::from_paths`] deliberately considers only files supplied
/// by the caller, making single-file linting safe: semantic findings are
/// emitted only when a complete layer, including its `conf/layer.conf`, is
/// present in the input set. [`WorkspaceIndex::from_build_dir`] instead
/// discovers the complete static scope declared by a build's `BBLAYERS`.
/// Layer metadata is used to build a deterministic BBPATH search order; layer
/// priority remains the fallback when the supplied metadata does not describe
/// a more specific path order.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceIndex {
    layers: Vec<LayerIndex>,
    search_paths: Vec<SearchPath>,
    dependencies: BTreeMap<DependencyNode, Vec<DependencyEdge>>,
    files: BTreeSet<PathBuf>,
    build_files: BTreeSet<PathBuf>,
    build_root: Option<PathBuf>,
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

        Self::from_canonical_files(files, BTreeSet::new(), roots)
    }

    /// Builds an index for every metadata file in a configured BitBake build.
    ///
    /// The build's `conf/bblayers.conf` is evaluated only for static
    /// `BBLAYERS` assignments. Each resolved layer is recursively indexed,
    /// while the build's own `conf` tree is included as global metadata. A
    /// dynamic or missing layer path is rejected instead of silently creating
    /// an incomplete workspace.
    pub fn from_build_dir(path: impl AsRef<Path>) -> io::Result<Self> {
        let build_dir = fs::canonicalize(path.as_ref())?;
        validate_build_dir(&build_dir)?;
        let layer_roots = parse_build_layers(&build_dir)?;
        let mut files = BTreeSet::new();
        for root in &layer_roots {
            files.extend(discover_metadata_files(root, MetadataTree::Layer)?);
        }
        let build_files = discover_metadata_files(&build_dir, MetadataTree::Build)?;
        if build_files.is_empty() {
            return Err(invalid_data(
                "build configuration contains no metadata files",
            ));
        }
        files.extend(build_files.iter().cloned());
        let roots = layer_roots.into_iter().collect::<BTreeSet<_>>();
        Self::from_canonical_files(files, build_files, roots)
    }

    fn from_canonical_files(
        files: BTreeSet<PathBuf>,
        build_files: BTreeSet<PathBuf>,
        roots: BTreeSet<PathBuf>,
    ) -> io::Result<Self> {
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
            dependencies: BTreeMap::new(),
            files,
            build_root: build_files
                .iter()
                .find_map(|path| path.parent()?.parent().map(Path::to_path_buf)),
            build_files,
        };
        index.search_paths = build_search_paths(&index.layers);
        index.dependencies = index.build_dependencies();
        Ok(index)
    }

    /// Returns every metadata file included in this workspace in stable path
    /// order.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(PathBuf::as_path)
    }

    /// Returns whether a path has enough whole-workspace context for
    /// workspace-aware lint rules.
    pub fn is_workspace_file(&self, path: &Path) -> bool {
        let path = canonicalize_for_lookup(path);
        self.build_files.contains(&path) || self.is_complete_for(&path)
    }

    /// Returns whether path belongs to a complete indexed layer.
    pub fn is_complete_for(&self, path: &Path) -> bool {
        let path = canonicalize_for_lookup(path);
        self.layers
            .iter()
            .any(|layer| layer.is_complete() && layer.files.contains(&path))
    }

    /// Returns every statically declared layer collection in the indexed
    /// workspace.
    ///
    /// The result is useful for validating `LAYERDEPENDS_*` without exposing
    /// the index's internal layer representation.
    pub fn collection_names(&self) -> BTreeSet<String> {
        self.layers
            .iter()
            .flat_map(|layer| layer.metadata.collections.iter().cloned())
            .collect()
    }

    /// Returns all matching recipe class definitions in BitBake search order.
    ///
    /// For inherit, BitBake searches all classes-recipe directories on
    /// BBPATH before searching the ordinary classes directories. The returned
    /// candidates retain layer, collection, priority, and scope information so
    /// callers can explain or validate the resolution.
    pub fn class_candidates(&self, class: &str) -> Vec<WorkspaceCandidate<'_>> {
        self.class_candidates_for(class, WorkspaceClassContext::Recipe)
    }

    /// Returns all matching static class definitions for a BitBake parse context.
    ///
    /// Global metadata searches `classes-global` before the shared `classes`
    /// directory. Recipe metadata searches `classes-recipe` before `classes`.
    pub fn class_candidates_for(
        &self,
        class: &str,
        context: WorkspaceClassContext,
    ) -> Vec<WorkspaceCandidate<'_>> {
        if class.is_empty() || class.contains(['/', '\\']) {
            return Vec::new();
        }

        let mut candidates = Vec::new();
        let scoped_directories = match context {
            WorkspaceClassContext::Global => [
                ("classes-global", WorkspaceSearchScope::ClassesGlobal),
                ("classes", WorkspaceSearchScope::Classes),
            ],
            WorkspaceClassContext::Recipe => [
                ("classes-recipe", WorkspaceSearchScope::ClassesRecipe),
                ("classes", WorkspaceSearchScope::Classes),
            ],
        };
        for (directory, scope) in scoped_directories {
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
        self.resolve_class_for(class, WorkspaceClassContext::Recipe)
    }

    /// Resolves a static class name for a BitBake parse context.
    pub fn resolve_class_for(&self, class: &str, context: WorkspaceClassContext) -> Option<&Path> {
        self.class_candidates_for(class, context)
            .into_iter()
            .next()
            .map(|candidate| candidate.path)
    }

    /// Returns the class namespace implied by an independently linted path.
    ///
    /// Configuration files and files in `classes-global` use global class
    /// lookup. Recipes, recipe classes, and otherwise ambiguous includes use
    /// recipe lookup. The dependency graph retains both contexts for shared
    /// includes and classes.
    pub fn class_context_for_path(&self, path: &Path) -> WorkspaceClassContext {
        self.class_contexts_for_path(path)[0]
    }

    /// Returns every class namespace in which a metadata file can be parsed.
    ///
    /// Generic classes and includes can be reached from either configuration
    /// or recipe parsing, so both contexts are returned for those paths.
    pub fn class_contexts_for_path(&self, path: &Path) -> Vec<WorkspaceClassContext> {
        dependency_contexts_for_path(&canonicalize_for_lookup(path)).to_vec()
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
                .candidate_for_path(target_path, WorkspaceSearchScope::CurrentFile)
                .into_iter()
                .collect();
        }

        let from = canonicalize_for_lookup(from);
        let mut candidates = Vec::new();

        if !matches!(directive, WorkspaceFileDirective::IncludeAll)
            && let Some(parent) = from.parent()
            && let Some(candidate) = self
                .candidate_for_path(&parent.join(target_path), WorkspaceSearchScope::CurrentFile)
        {
            append_candidate(&mut candidates, candidate);
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

    /// Returns the resolved static dependency edges originating at `path`.
    ///
    /// Edges are built only for complete supplied layers. Dynamic directive
    /// arguments and unresolved optional includes are deliberately omitted.
    pub fn dependencies_from(&self, path: &Path) -> Vec<WorkspaceDependency<'_>> {
        let path = canonicalize_for_lookup(path);
        let mut dependencies = Vec::new();
        let mut seen = BTreeSet::new();
        for context in dependency_contexts_for_path(&path) {
            let lookup = DependencyNode {
                path: path.clone(),
                context: *context,
            };
            let Some((from, edges)) = self.dependencies.get_key_value(&lookup) else {
                continue;
            };
            for edge in edges {
                if seen.insert((edge.kind, edge.to.path.as_path())) {
                    dependencies.push(WorkspaceDependency {
                        from: from.path.as_path(),
                        to: edge.to.path.as_path(),
                        kind: edge.kind,
                    });
                }
            }
        }
        dependencies
    }

    /// Returns a deterministic cycle witness if a resolved edge from `from`
    /// to `to` closes a static workspace dependency cycle.
    ///
    /// The returned path starts and ends at `from`, with `to` as its second
    /// entry. An empty or dynamically unresolved graph never produces a
    /// cycle witness.
    pub fn dependency_cycle(&self, from: &Path, to: &Path) -> Option<Vec<PathBuf>> {
        self.class_contexts_for_path(from)
            .into_iter()
            .find_map(|context| self.dependency_cycle_for(from, to, context))
    }

    /// Returns a deterministic cycle witness in a specific BitBake parse context.
    pub fn dependency_cycle_for(
        &self,
        from: &Path,
        to: &Path,
        context: WorkspaceClassContext,
    ) -> Option<Vec<PathBuf>> {
        let from = canonicalize_for_lookup(from);
        let to = canonicalize_for_lookup(to);
        if from == to {
            return Some(vec![from.clone(), from]);
        }

        let from = DependencyNode {
            path: from,
            context,
        };
        let to = DependencyNode { path: to, context };
        let mut queue = VecDeque::from([to.clone()]);
        let mut visited = BTreeSet::from([to.clone()]);
        let mut parents = BTreeMap::new();

        while let Some(current) = queue.pop_front() {
            let Some(edges) = self.dependencies.get(&current) else {
                continue;
            };
            for edge in edges {
                let next = edge.to.clone();
                if !visited.insert(next.clone()) {
                    continue;
                }
                parents.insert(next.clone(), current.clone());
                if next == from {
                    let mut tail = vec![from.clone()];
                    while tail.last() != Some(&to) {
                        let parent = parents.get(tail.last().unwrap())?.clone();
                        tail.push(parent);
                    }
                    tail.reverse();
                    let mut cycle = vec![from];
                    cycle.extend(tail);
                    return Some(cycle.into_iter().map(|node| node.path).collect());
                }
                queue.push_back(next);
            }
        }
        None
    }

    fn build_dependencies(&self) -> BTreeMap<DependencyNode, Vec<DependencyEdge>> {
        let mut dependencies = BTreeMap::new();
        for path in &self.files {
            if !self.is_workspace_file(path) {
                continue;
            }
            let Ok(source) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(tree) = parse(&source) else {
                continue;
            };
            for context in dependency_contexts_for_path(path) {
                let edges = collect_dependency_edges(self, path, &tree, *context);
                if !edges.is_empty() {
                    dependencies.insert(
                        DependencyNode {
                            path: path.clone(),
                            context: *context,
                        },
                        edges,
                    );
                }
            }
        }
        dependencies
    }

    fn candidate_for_path<'a>(
        &'a self,
        candidate: &Path,
        scope: WorkspaceSearchScope,
    ) -> Option<WorkspaceCandidate<'a>> {
        if let Some((index, path)) = self.find_layer_path(candidate) {
            return Some(self.layers[index].make_candidate(path, scope));
        }
        let canonical = canonicalize_for_lookup(candidate);
        self.build_files.get(&canonical).and_then(|path| {
            self.build_root.as_deref().map(|root| WorkspaceCandidate {
                path,
                layer: root,
                priority: DEFAULT_LAYER_PRIORITY,
                collection: None,
                scope,
            })
        })
    }

    fn find_layer_path<'a>(&'a self, candidate: &Path) -> Option<(usize, &'a Path)> {
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyNode {
    path: PathBuf,
    context: WorkspaceClassContext,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyEdge {
    to: DependencyNode,
    kind: WorkspaceDependencyKind,
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

fn collect_dependency_edges(
    index: &WorkspaceIndex,
    from: &Path,
    tree: &SyntaxTree<'_>,
    context: WorkspaceClassContext,
) -> Vec<DependencyEdge> {
    let mut edges = Vec::new();

    for node in tree.nodes() {
        match node.kind() {
            SyntaxKind::Directive(directive) => {
                let arguments = directive.arguments();
                let arguments = &arguments[..comment_start(arguments).unwrap_or(arguments.len())];

                match directive.keyword() {
                    DirectiveKeyword::Include
                    | DirectiveKeyword::IncludeAll
                    | DirectiveKeyword::Require => {
                        let (kind, directive_kind, include_all) = match directive.keyword() {
                            DirectiveKeyword::Include => (
                                WorkspaceDependencyKind::Include,
                                WorkspaceFileDirective::Include,
                                false,
                            ),
                            DirectiveKeyword::IncludeAll => (
                                WorkspaceDependencyKind::IncludeAll,
                                WorkspaceFileDirective::IncludeAll,
                                true,
                            ),
                            DirectiveKeyword::Require => (
                                WorkspaceDependencyKind::Require,
                                WorkspaceFileDirective::Require,
                                false,
                            ),
                            _ => unreachable!("matched only file dependency directives"),
                        };
                        for target in static_directive_words(arguments) {
                            let candidates =
                                index.file_candidates_for(from, target, directive_kind);
                            if include_all {
                                edges.extend(candidates.into_iter().map(|candidate| {
                                    dependency_edge(candidate.path(), context, kind)
                                }));
                            } else if let Some(candidate) = candidates.first() {
                                edges.push(dependency_edge(candidate.path(), context, kind));
                            }
                        }
                    }
                    DirectiveKeyword::Inherit | DirectiveKeyword::InheritDefer => {
                        let kind = if matches!(directive.keyword(), DirectiveKeyword::Inherit) {
                            WorkspaceDependencyKind::Inherit
                        } else {
                            WorkspaceDependencyKind::InheritDefer
                        };
                        for class in static_directive_words(arguments) {
                            if let Some(candidate) = index
                                .class_candidates_for(class, context)
                                .into_iter()
                                .next()
                            {
                                edges.push(dependency_edge(candidate.path(), context, kind));
                            }
                        }
                    }
                    _ => {}
                }
            }
            SyntaxKind::Assignment(assignment) if context == WorkspaceClassContext::Global => {
                let Some(kind) = global_class_assignment_kind(assignment.name()) else {
                    continue;
                };
                for class in static_assignment_words(assignment.value()) {
                    if let Some(candidate) = index
                        .class_candidates_for(&class, context)
                        .into_iter()
                        .next()
                    {
                        edges.push(dependency_edge(candidate.path(), context, kind));
                    }
                }
            }
            _ => {}
        }
    }

    edges.sort();
    edges.dedup();
    edges
}

pub(crate) fn global_class_assignment_kind(name: &str) -> Option<WorkspaceDependencyKind> {
    let mut components = name.split(':');
    let base_name = components.next()?;
    if components.any(|component| component == "remove") {
        return None;
    }
    match base_name {
        "INHERIT" => Some(WorkspaceDependencyKind::InheritGlobal),
        "USER_CLASSES" => Some(WorkspaceDependencyKind::UserClasses),
        _ => None,
    }
}

fn dependency_edge(
    path: &Path,
    context: WorkspaceClassContext,
    kind: WorkspaceDependencyKind,
) -> DependencyEdge {
    DependencyEdge {
        to: DependencyNode {
            path: path.to_path_buf(),
            context,
        },
        kind,
    }
}

fn dependency_contexts_for_path(path: &Path) -> &'static [WorkspaceClassContext] {
    const GLOBAL: &[WorkspaceClassContext] = &[WorkspaceClassContext::Global];
    const RECIPE: &[WorkspaceClassContext] = &[WorkspaceClassContext::Recipe];
    const SHARED: &[WorkspaceClassContext] =
        &[WorkspaceClassContext::Recipe, WorkspaceClassContext::Global];

    if path.extension().and_then(|extension| extension.to_str()) == Some("conf")
        || path
            .components()
            .any(|component| component.as_os_str() == "classes-global")
    {
        GLOBAL
    } else if matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("bb" | "bbappend")
    ) || path
        .components()
        .any(|component| component.as_os_str() == "classes-recipe")
    {
        RECIPE
    } else {
        SHARED
    }
}

#[derive(Clone, Copy)]
enum MetadataTree {
    Layer,
    Build,
}

fn validate_build_dir(build_dir: &Path) -> io::Result<()> {
    let metadata = fs::metadata(build_dir)?;
    if !metadata.is_dir() {
        return Err(invalid_data(format!(
            "BitBake build path {} is not a directory",
            build_dir.display()
        )));
    }
    for file in ["conf/local.conf", "conf/bblayers.conf"] {
        if !build_dir.join(file).is_file() {
            return Err(invalid_data(format!(
                "BitBake build directory is missing {file}"
            )));
        }
    }
    Ok(())
}

fn parse_build_layers(build_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let path = build_dir.join("conf/bblayers.conf");
    let source = fs::read_to_string(&path)?;
    let tree = parse(&source)
        .map_err(|error| invalid_data(format!("could not parse {}: {error}", path.display())))?;
    let mut layers = Vec::new();

    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        if assignment.name().split(':').next() != Some("BBLAYERS") {
            continue;
        }
        let values = static_build_layer_paths(assignment.value(), build_dir)?;
        let override_components = assignment.name().split(':').skip(1).collect::<Vec<_>>();
        let is_append_override = override_components.contains(&"append");
        let is_prepend_override = override_components.contains(&"prepend");
        let is_remove = override_components.contains(&"remove");
        if is_remove {
            layers.retain(|layer| !values.contains(layer));
            continue;
        }
        let operator = if is_append_override {
            AssignmentOperator::AppendWithSpace
        } else if is_prepend_override {
            AssignmentOperator::PrependWithSpace
        } else {
            assignment.operator()
        };
        match operator {
            AssignmentOperator::Assign | AssignmentOperator::Immediate => {
                layers = values;
            }
            AssignmentOperator::Default | AssignmentOperator::WeakDefault => {
                if layers.is_empty() {
                    layers = values;
                }
            }
            AssignmentOperator::AppendWithSpace | AssignmentOperator::AppendWithoutSpace => {
                layers.extend(values);
            }
            AssignmentOperator::PrependWithSpace | AssignmentOperator::PrependWithoutSpace => {
                let mut combined = values;
                combined.extend(std::mem::take(&mut layers));
                layers = combined;
            }
        }
    }

    if layers.is_empty() {
        return Err(invalid_data(format!(
            "{} does not contain a static non-empty BBLAYERS value",
            path.display()
        )));
    }

    let mut resolved = Vec::new();
    for layer in layers {
        let layer = fs::canonicalize(&layer).map_err(|error| {
            invalid_data(format!(
                "BBLAYERS entry {} could not be resolved: {error}",
                layer.display()
            ))
        })?;
        if !layer.join("conf/layer.conf").is_file() {
            return Err(invalid_data(format!(
                "BBLAYERS entry {} is not a layer with conf/layer.conf",
                layer.display()
            )));
        }
        if !resolved.contains(&layer) {
            resolved.push(layer);
        }
    }
    Ok(resolved)
}

fn static_build_layer_paths(value: &str, build_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let value = scalar_value(value).ok_or_else(|| invalid_data("BBLAYERS has no scalar value"))?;
    let mut paths = Vec::new();
    for word in value.split_ascii_whitespace() {
        if word == "\\" {
            continue;
        }
        let word = expand_build_variables(word, build_dir)?;
        if word.is_empty() {
            continue;
        }
        let path = PathBuf::from(word);
        paths.push(if path.is_absolute() {
            path
        } else {
            build_dir.join(path)
        });
    }
    Ok(paths)
}

fn expand_build_variables(value: &str, build_dir: &Path) -> io::Result<String> {
    let mut expanded = value.to_owned();
    loop {
        let Some(start) = expanded.find("${") else {
            break;
        };
        let end = expanded[start + 2..]
            .find('}')
            .map(|relative| start + 2 + relative)
            .ok_or_else(|| {
                invalid_data(format!("unterminated variable in BBLAYERS entry '{value}'"))
            })?;
        let name = &expanded[start + 2..end];
        if name.starts_with('@') {
            return Err(invalid_data(format!(
                "BBLAYERS entry '{value}' uses dynamic expansion"
            )));
        }
        let replacement = match name {
            "TOPDIR" | "BUILDDIR" | "PWD" => build_dir.to_string_lossy().into_owned(),
            _ => std::env::var(name).map_err(|_| {
                invalid_data(format!(
                    "BBLAYERS entry '{value}' uses unresolved variable ${{{name}}}"
                ))
            })?,
        };
        expanded.replace_range(start..=end, &replacement);
    }
    if expanded.contains('$') {
        return Err(invalid_data(format!(
            "BBLAYERS entry '{value}' uses dynamic expansion"
        )));
    }
    if let Some(home) = std::env::var_os("HOME")
        && (expanded == "~" || expanded.starts_with("~/"))
    {
        expanded = home.to_string_lossy().to_string() + &expanded[1..];
    }
    Ok(expanded)
}

fn discover_metadata_files(root: &Path, tree: MetadataTree) -> io::Result<BTreeSet<PathBuf>> {
    let start = match tree {
        MetadataTree::Layer => root.to_path_buf(),
        MetadataTree::Build => root.join("conf"),
    };
    if !start.is_dir() {
        return Err(invalid_data(format!(
            "metadata root {} is not a directory",
            start.display()
        )));
    }
    let mut files = BTreeSet::new();
    walk_metadata_files(&start, root, tree, &mut files)?;
    Ok(files)
}

fn walk_metadata_files(
    directory: &Path,
    root: &Path,
    tree: MetadataTree,
    files: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if path.file_name().is_some_and(|name| name == "files") {
                continue;
            }
            walk_metadata_files(&path, root, tree, files)?;
        } else if file_type.is_file() && is_workspace_metadata_file(&path, root, tree) {
            files.insert(path);
        }
    }
    Ok(())
}

fn is_workspace_metadata_file(path: &Path, root: &Path, tree: MetadataTree) -> bool {
    const EXTENSIONS: &[&str] = &["bb", "bbappend", "bbclass", "conf", "inc"];
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension))
    {
        return false;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative
        .components()
        .any(|component| component.as_os_str() == "files")
    {
        return false;
    }
    match tree {
        MetadataTree::Build => true,
        MetadataTree::Layer => {
            path.extension().and_then(|extension| extension.to_str()) != Some("conf")
                || path.ancestors().any(|ancestor| {
                    ancestor.file_name().is_some_and(|name| name == "conf")
                        && ancestor.join("layer.conf").is_file()
                })
        }
    }
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
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

fn static_directive_words(text: &str) -> Vec<&str> {
    let mut dynamic_expression = false;
    let mut words = Vec::new();
    for word in text.split_ascii_whitespace() {
        if dynamic_expression {
            dynamic_expression = !word.contains('}');
            continue;
        }
        if word.contains('$') || word.contains('{') {
            dynamic_expression = !word.contains('}');
            continue;
        }
        if word == "\\" || word.contains('}') {
            continue;
        }
        words.push(word);
    }
    words
}

fn static_assignment_words(value: &str) -> Vec<String> {
    let Some(value) = scalar_value(value) else {
        return Vec::new();
    };
    static_directive_words(&value)
        .into_iter()
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
