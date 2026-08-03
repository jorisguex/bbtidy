use bbtidy::{
    WorkspaceDependencyKind, WorkspaceFileDirective, WorkspaceIndex, WorkspaceSearchScope,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn indexes_complete_layers_and_resolves_classes_and_files() {
    let layer = TemporaryLayer::new("workspace-index");
    let layer_conf = layer.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
    let class = layer.write("classes/base.bbclass", "BASE = \"1\"\n");
    let include = layer.write("recipes-example/common.inc", "COMMON = \"1\"\n");
    let recipe = layer.write(
        "recipes-example/example.bb",
        "require common.inc\ninherit base\n",
    );

    let index =
        WorkspaceIndex::from_paths([layer_conf, class.clone(), include.clone(), recipe.clone()])
            .unwrap();
    let canonical_class = fs::canonicalize(class).unwrap();
    let canonical_include = fs::canonicalize(include).unwrap();

    assert!(index.is_complete_for(&recipe));
    assert_eq!(index.resolve_class("base"), Some(canonical_class.as_path()));
    assert_eq!(
        index.resolve_file(&recipe, "common.inc"),
        Some(canonical_include.as_path())
    );
    assert!(index.resolve_file(&recipe, "missing.inc").is_none());
}

#[test]
fn builds_static_dependency_edges_and_reports_cycle_witnesses() {
    let layer = TemporaryLayer::new("workspace-dependency-graph");
    let layer_conf = layer.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
    let class = layer.write(
        "classes/base.bbclass",
        "require recipes-example/example/example.bb\n",
    );
    let helper = layer.write("recipes-example/example/helper.inc", "require example.bb\n");
    let required = layer.write(
        "recipes-example/example/required.inc",
        "require example.bb\n",
    );
    let shared = layer.write("shared.inc", "require recipes-example/example/example.bb\n");
    let recipe = layer.write(
        "recipes-example/example/example.bb",
        concat!(
            "include helper.inc\n",
            "include_all shared.inc\n",
            "require required.inc\n",
            "inherit base\n",
            "include ${DYNAMIC}\n",
        ),
    );

    let index = WorkspaceIndex::from_paths([
        layer_conf,
        class.clone(),
        helper.clone(),
        required.clone(),
        shared.clone(),
        recipe.clone(),
    ])
    .unwrap();

    let mut dependencies = index
        .dependencies_from(&recipe)
        .into_iter()
        .map(|dependency| {
            assert_eq!(dependency.from(), fs::canonicalize(&recipe).unwrap());
            (dependency.kind(), dependency.to().to_path_buf())
        })
        .collect::<Vec<_>>();
    dependencies.sort();
    assert_eq!(
        dependencies,
        vec![
            (
                WorkspaceDependencyKind::Include,
                fs::canonicalize(&helper).unwrap(),
            ),
            (
                WorkspaceDependencyKind::IncludeAll,
                fs::canonicalize(&shared).unwrap(),
            ),
            (
                WorkspaceDependencyKind::Require,
                fs::canonicalize(&required).unwrap(),
            ),
            (
                WorkspaceDependencyKind::Inherit,
                fs::canonicalize(&class).unwrap(),
            ),
        ]
    );

    let cycle = index
        .dependency_cycle(&recipe, &helper)
        .expect("helper should require its source recipe");
    assert_eq!(cycle.first(), cycle.last());
    assert_eq!(cycle.first(), Some(&fs::canonicalize(recipe).unwrap()));
    assert_eq!(cycle.get(1), Some(&fs::canonicalize(helper).unwrap()));
}

#[test]
fn incomplete_file_contexts_do_not_appear_complete() {
    let layer = TemporaryLayer::new("workspace-incomplete");
    layer.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
    let recipe = layer.write("recipes-example/example.bb", "inherit base\n");
    let index = WorkspaceIndex::from_paths([recipe.clone()]).unwrap();

    assert!(!index.is_complete_for(&recipe));
    assert!(index.resolve_class("base").is_none());
    assert!(index.dependencies_from(&recipe).is_empty());
}

#[test]
fn resolves_classes_by_layer_priority_and_retains_candidates() {
    let low = TemporaryLayer::new("workspace-low-priority");
    let high = TemporaryLayer::new("workspace-high-priority");
    let low_conf = low.write("conf/layer.conf", "BBFILE_PRIORITY_low = \"5\"\n");
    let high_conf = high.write("conf/layer.conf", "BBFILE_PRIORITY_high = \"10\"\n");
    let low_class = low.write("classes/base.bbclass", "BASE = \"low\"\n");
    let high_class = high.write("classes/base.bbclass", "BASE = \"high\"\n");
    let low_include = low.write("common.inc", "COMMON = \"low\"\n");
    let high_include = high.write("common.inc", "COMMON = \"high\"\n");
    let consumer = TemporaryLayer::new("workspace-consumer");
    let consumer_conf = consumer.write("conf/layer.conf", "BBFILE_PRIORITY_consumer = \"1\"\n");
    let consumer_recipe = consumer.write("recipes-example/example.bb", "inherit base\n");

    let index = WorkspaceIndex::from_paths([
        low_conf,
        high_conf,
        low_class.clone(),
        high_class.clone(),
        low_include,
        high_include.clone(),
        consumer_conf,
        consumer_recipe.clone(),
    ])
    .unwrap();
    let canonical_high = fs::canonicalize(high_class).unwrap();
    let canonical_low = fs::canonicalize(low_class).unwrap();

    assert_eq!(index.resolve_class("base"), Some(canonical_high.as_path()));
    let canonical_high_include = fs::canonicalize(high_include).unwrap();
    assert_eq!(
        index.resolve_file(&consumer_recipe, "common.inc"),
        Some(canonical_high_include.as_path())
    );
    assert_eq!(
        index
            .class_candidates("base")
            .iter()
            .map(|candidate| (candidate.path(), candidate.priority()))
            .collect::<Vec<_>>(),
        vec![(canonical_high.as_path(), 10), (canonical_low.as_path(), 5),]
    );
}

#[test]
fn follows_bbpath_collection_metadata_and_include_modes() {
    let low = TemporaryLayer::new("workspace-bbpath-low");
    let high = TemporaryLayer::new("workspace-bbpath-high");
    let consumer = TemporaryLayer::new("workspace-bbpath-consumer");
    let low_conf = low.write(
        "conf/layer.conf",
        concat!(
            "BBFILE_COLLECTIONS += \"low\"\n",
            "BBFILE_PATTERN_low = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_low = \"5\"\n",
            "BBPATH .= \":${LAYERDIR}\"\n",
        ),
    );
    let high_conf = high.write(
        "conf/layer.conf",
        concat!(
            "BBFILE_COLLECTIONS += \"high\"\n",
            "BBFILE_PATTERN_high = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_high = \"10\"\n",
            "BBPATH .= \":${LAYERDIR}\"\n",
        ),
    );
    let consumer_conf = consumer.write(
        "conf/layer.conf",
        concat!(
            "BBFILE_COLLECTIONS += \"consumer\"\n",
            "BBFILE_PATTERN_consumer = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_consumer = \"1\"\n",
            "BBPATH .= \":${LAYERDIR}\"\n",
        ),
    );
    let low_include = low.write("shared.inc", "ORIGIN = \"low\"\n");
    let high_include = high.write("shared.inc", "ORIGIN = \"high\"\n");
    let local_include = consumer.write("recipes-example/local.inc", "ORIGIN = \"local\"\n");
    let recipe = consumer.write("recipes-example/example.bb", "require shared.inc\n");

    let index = WorkspaceIndex::from_paths([
        low_conf,
        high_conf,
        consumer_conf,
        low_include,
        high_include.clone(),
        local_include.clone(),
        recipe.clone(),
    ])
    .unwrap();

    let require_candidates =
        index.file_candidates_for(&recipe, "shared.inc", WorkspaceFileDirective::Require);
    assert_eq!(require_candidates.len(), 2);
    assert_eq!(
        require_candidates[0].path(),
        fs::canonicalize(&high_include).unwrap()
    );
    assert_eq!(require_candidates[0].collection(), Some("high"));
    assert_eq!(require_candidates[0].scope(), WorkspaceSearchScope::Bbpath);
    assert_eq!(require_candidates[1].priority(), 5);
    let dependencies = index.dependencies_from(&recipe);
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].kind(), WorkspaceDependencyKind::Require);
    assert_eq!(
        dependencies[0].to(),
        fs::canonicalize(&high_include).unwrap().as_path()
    );

    let include_all = index.include_all_candidates(&recipe, "shared.inc");
    assert_eq!(include_all.len(), 2);
    assert!(
        include_all
            .iter()
            .all(|candidate| candidate.scope() == WorkspaceSearchScope::Bbpath)
    );

    let local_candidates =
        index.file_candidates_for(&recipe, "local.inc", WorkspaceFileDirective::Include);
    assert_eq!(local_candidates.len(), 1);
    assert_eq!(
        local_candidates[0].path(),
        fs::canonicalize(local_include).unwrap()
    );
    assert_eq!(
        local_candidates[0].scope(),
        WorkspaceSearchScope::CurrentFile
    );

    let include_all_local = index.include_all_candidates(&recipe, "local.inc");
    assert!(include_all_local.is_empty());
}

#[test]
fn class_search_prefers_classes_recipe_before_classes() {
    let recipe_layer = TemporaryLayer::new("workspace-class-scope-recipe");
    let global_layer = TemporaryLayer::new("workspace-class-scope-global");
    let recipe_conf = recipe_layer.write("conf/layer.conf", "BBFILE_PRIORITY_recipe = \"5\"\n");
    let global_conf = global_layer.write("conf/layer.conf", "BBFILE_PRIORITY_global = \"10\"\n");
    let recipe_class = recipe_layer.write("classes-recipe/base.bbclass", "ORIGIN = \"recipe\"\n");
    let global_class = global_layer.write("classes/base.bbclass", "ORIGIN = \"global\"\n");

    let index =
        WorkspaceIndex::from_paths([recipe_conf, global_conf, recipe_class.clone(), global_class])
            .unwrap();

    assert_eq!(
        index.resolve_class("base"),
        Some(fs::canonicalize(recipe_class).unwrap().as_path())
    );
    assert_eq!(
        index.class_candidates("base")[0].scope(),
        WorkspaceSearchScope::ClassesRecipe
    );
}

struct TemporaryLayer {
    root: PathBuf,
}

impl TemporaryLayer {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("bbtidy-{label}-{}-{timestamp}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TemporaryLayer {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(Path::new(&self.root));
    }
}
