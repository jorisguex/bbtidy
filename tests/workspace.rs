use bbtidy::{
    WorkspaceClassContext, WorkspaceDependencyKind, WorkspaceFileDirective, WorkspaceIndex,
    WorkspaceSearchScope,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
fn indexes_every_layer_and_build_configuration_from_bblayers() {
    let project = TemporaryLayer::new("workspace-build");
    let first = project.root.join("meta-first");
    let second = project.root.join("meta-second");
    let build = project.root.join("build");

    fs::create_dir_all(build.join("conf")).unwrap();
    fs::write(
        build.join("conf/local.conf"),
        "require build.inc\nMACHINE = \"qemux86-64\"\n",
    )
    .unwrap();
    let build_include = build.join("conf/build.inc");
    fs::write(&build_include, "BUILD_CONTEXT = \"1\"\n").unwrap();
    fs::write(
        build.join("conf/bblayers.conf"),
        "BBLAYERS ?= \"${TOPDIR}/../meta-first\"\nBBLAYERS:append = \"${TOPDIR}/../meta-second\"\n",
    )
    .unwrap();

    let first_conf = write_path(
        &first,
        "conf/layer.conf",
        "BBFILE_COLLECTIONS += \"first\"\nBBFILE_PRIORITY_first = \"5\"\nBBPATH .= \":${LAYERDIR}\"\n",
    );
    write_path(&first, "recipes-example/example.bb", "inherit shared\n");
    let second_conf = write_path(
        &second,
        "conf/layer.conf",
        "BBFILE_COLLECTIONS += \"second\"\nBBFILE_PRIORITY_second = \"10\"\nBBPATH .= \":${LAYERDIR}\"\n",
    );
    let shared = write_path(&second, "classes/shared.bbclass", "SHARED = \"1\"\n");

    let index = WorkspaceIndex::from_build_dir(&build).unwrap();
    let files = index.files().collect::<BTreeSet<_>>();
    assert!(
        files.contains(
            &fs::canonicalize(build.join("conf/local.conf"))
                .unwrap()
                .as_path()
        )
    );
    assert!(
        files.contains(
            &fs::canonicalize(build.join("conf/bblayers.conf"))
                .unwrap()
                .as_path()
        )
    );
    assert!(files.contains(&fs::canonicalize(&build_include).unwrap().as_path()));
    assert!(files.contains(&fs::canonicalize(&first_conf).unwrap().as_path()));
    assert!(files.contains(&fs::canonicalize(&second_conf).unwrap().as_path()));
    assert!(files.contains(&fs::canonicalize(&shared).unwrap().as_path()));

    let recipe = fs::canonicalize(first.join("recipes-example/example.bb")).unwrap();
    assert!(index.is_workspace_file(&build.join("conf/local.conf")));
    assert!(index.is_workspace_file(&recipe));
    assert_eq!(
        index.resolve_class("shared"),
        Some(fs::canonicalize(shared).unwrap().as_path())
    );
    assert_eq!(
        index.dependencies_from(&build.join("conf/local.conf"))[0].to(),
        fs::canonicalize(build_include).unwrap().as_path()
    );
}

#[test]
fn rejects_dynamic_or_missing_bblayers_entries() {
    let project = TemporaryLayer::new("workspace-build-invalid");
    let build = project.root.join("build");
    fs::create_dir_all(build.join("conf")).unwrap();
    fs::write(build.join("conf/local.conf"), "MACHINE = \"qemux86-64\"\n").unwrap();
    fs::write(
        build.join("conf/bblayers.conf"),
        "BBLAYERS = \"${@get_layers(d)}\"\n",
    )
    .unwrap();

    let error = WorkspaceIndex::from_build_dir(&build).unwrap_err();
    assert!(error.to_string().contains("dynamic expansion"));
}

#[cfg(unix)]
#[test]
fn rejects_symbolic_link_build_directories() {
    use std::os::unix::fs::symlink;

    let project = TemporaryLayer::new("workspace-build-symlink");
    let build = project.root.join("build");
    fs::create_dir_all(build.join("conf")).unwrap();
    fs::write(build.join("conf/local.conf"), "MACHINE = \"qemux86-64\"\n").unwrap();
    fs::write(build.join("conf/bblayers.conf"), "BBLAYERS = \"\"\n").unwrap();
    let link = project.root.join("build-link");
    symlink(&build, &link).unwrap();

    let error = WorkspaceIndex::from_build_dir(&link).unwrap_err();
    assert!(error.to_string().contains("symbolic link"));
}

#[cfg(unix)]
#[test]
fn bitbake_index_uses_engine_resolved_dynamic_metadata() {
    let project = TemporaryLayer::new("workspace-bitbake-resolved");
    let layer = project.root.join("meta-dynamic");
    let build = project.root.join("build");
    let layer = fs::canonicalize(&layer).unwrap_or(layer);
    fs::create_dir_all(build.join("conf")).unwrap();
    fs::write(build.join("conf/local.conf"), "MACHINE = \"qemux86-64\"\n").unwrap();
    fs::write(
        build.join("conf/bblayers.conf"),
        "BBLAYERS = \"${@compute_layers(d)}\"\n",
    )
    .unwrap();
    fs::create_dir_all(layer.join("conf")).unwrap();
    fs::write(
        layer.join("conf/layer.conf"),
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "BBFILE_COLLECTIONS += \"dynamic\"\n",
            "BBFILE_PATTERN_dynamic = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_dynamic = \"7\"\n",
        ),
    )
    .unwrap();
    let recipe = project.write(
        "meta-dynamic/recipes-example/example.bb",
        concat!(
            "inherit ${@select_class(d)}\n",
            "require ${@select_include(d)}\n",
            "SUMMARY = \"example\"\n",
        ),
    );
    let append = project.write(
        "meta-dynamic/recipes-example/example.bbappend",
        "SUMMARY:append = \" via append\"\n",
    );
    let dynamic_class = project.write(
        "meta-dynamic/classes/dynamic.bbclass",
        "DYNAMIC_CLASS = \"1\"\n",
    );
    let dynamic_include = project.write("meta-dynamic/dynamic.inc", "DYNAMIC_INCLUDE = \"1\"\n");
    let build = fs::canonicalize(build).unwrap();
    let layer = fs::canonicalize(layer).unwrap();
    let recipe = fs::canonicalize(recipe).unwrap();
    let append = fs::canonicalize(append).unwrap();
    let dynamic_class = fs::canonicalize(dynamic_class).unwrap();
    let dynamic_include = fs::canonicalize(dynamic_include).unwrap();

    let bitbake = project.write(
        "fake-bitbake",
        &format!(
            r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.18.0'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  exit 0
fi
if [ "$1" = "--environment" ] && [ "$2" = "--buildfile" ]; then
  printf '%s\n' 'BBINCLUDED="{layer}/conf/layer.conf {dynamic_class} {dynamic_include}"'
  exit 0
fi
if [ "$1" = "--environment" ]; then
  printf '%s\n' 'BBLAYERS="{layer}"'
  printf '%s\n' 'BBPATH="{layer}"'
  printf '%s\n' 'BBFILES="{layer}/recipes-example/*.bb {layer}/recipes-example/*.bbappend"'
  printf '%s\n' 'BBINCLUDED="{build}/conf/local.conf {build}/conf/bblayers.conf {layer}/conf/layer.conf"'
  printf '%s\n' 'BBFILE_COLLECTIONS="dynamic"'
  printf '%s\n' 'BBFILE_PATTERN_dynamic="^{layer}/"'
  printf '%s\n' 'BBFILE_PRIORITY_dynamic="7"'
  exit 0
fi
exit 1
"###,
            layer = layer.display(),
            build = build.display(),
            dynamic_class = dynamic_class.display(),
            dynamic_include = dynamic_include.display(),
        ),
    );
    let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&bitbake, permissions).unwrap();

    let index = WorkspaceIndex::from_bitbake(&build, &bitbake).unwrap();

    assert!(index.is_workspace_file(&recipe));
    assert!(index.is_workspace_file(&append));
    assert!(index.is_workspace_file(&dynamic_class));
    assert!(index.is_workspace_file(&dynamic_include));
    assert_eq!(
        index.resolve_class("dynamic"),
        Some(dynamic_class.as_path())
    );
    let candidates = index.class_candidates("dynamic");
    let candidate = candidates.first().unwrap();
    assert_eq!(candidate.collection(), Some("dynamic"));
    assert_eq!(candidate.priority(), 7);
    assert_eq!(
        index.resolve_file(&recipe, "dynamic.inc"),
        Some(dynamic_include.as_path())
    );
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

#[test]
fn separates_global_and_recipe_class_namespaces_and_dependencies() {
    let layer = TemporaryLayer::new("workspace-global-class-scope");
    let layer_conf = layer.write(
        "conf/layer.conf",
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "INHERIT += \"base ${DYNAMIC}\"\n",
            "INHERIT:remove = \"removed\"\n",
            "USER_CLASSES += \"metrics\"\n",
        ),
    );
    let global_base = layer.write("classes-global/base.bbclass", "inherit helper\n");
    let global_helper = layer.write("classes-global/helper.bbclass", "require conf/layer.conf\n");
    let metrics = layer.write("classes/metrics.bbclass", "ORIGIN = \"shared\"\n");
    let removed = layer.write("classes-global/removed.bbclass", "ORIGIN = \"removed\"\n");
    let recipe_base = layer.write("classes-recipe/base.bbclass", "inherit helper\n");
    let recipe_helper = layer.write("classes-recipe/helper.bbclass", "ORIGIN = \"recipe\"\n");
    let shared_base = layer.write("classes/base.bbclass", "ORIGIN = \"shared\"\n");
    let recipe = layer.write("recipes-example/example.bb", "inherit base\n");

    let index = WorkspaceIndex::from_paths([
        layer_conf.clone(),
        global_base.clone(),
        global_helper.clone(),
        metrics.clone(),
        removed,
        recipe_base.clone(),
        recipe_helper.clone(),
        shared_base.clone(),
        recipe.clone(),
    ])
    .unwrap();

    let global_candidates = index.class_candidates_for("base", WorkspaceClassContext::Global);
    assert_eq!(
        global_candidates
            .iter()
            .map(|candidate| candidate.scope())
            .collect::<Vec<_>>(),
        [
            WorkspaceSearchScope::ClassesGlobal,
            WorkspaceSearchScope::Classes
        ]
    );
    assert_eq!(
        global_candidates[0].path(),
        fs::canonicalize(&global_base).unwrap()
    );

    let recipe_candidates = index.class_candidates_for("base", WorkspaceClassContext::Recipe);
    assert_eq!(
        recipe_candidates
            .iter()
            .map(|candidate| candidate.scope())
            .collect::<Vec<_>>(),
        [
            WorkspaceSearchScope::ClassesRecipe,
            WorkspaceSearchScope::Classes
        ]
    );
    assert_eq!(
        recipe_candidates[0].path(),
        fs::canonicalize(&recipe_base).unwrap()
    );
    assert_eq!(
        index.resolve_class("base"),
        Some(fs::canonicalize(&recipe_base).unwrap().as_path())
    );
    assert_eq!(
        index.class_context_for_path(&layer_conf),
        WorkspaceClassContext::Global
    );
    assert_eq!(
        index.class_context_for_path(&global_base),
        WorkspaceClassContext::Global
    );
    assert_eq!(
        index.class_context_for_path(&recipe),
        WorkspaceClassContext::Recipe
    );
    assert_eq!(
        index.class_contexts_for_path(&shared_base),
        [WorkspaceClassContext::Recipe, WorkspaceClassContext::Global]
    );

    let configuration_dependencies = index
        .dependencies_from(&layer_conf)
        .into_iter()
        .map(|dependency| (dependency.kind(), dependency.to().to_path_buf()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        configuration_dependencies,
        BTreeSet::from([
            (
                WorkspaceDependencyKind::InheritGlobal,
                fs::canonicalize(&global_base).unwrap(),
            ),
            (
                WorkspaceDependencyKind::UserClasses,
                fs::canonicalize(&metrics).unwrap(),
            ),
        ])
    );

    let global_dependencies = index.dependencies_from(&global_base);
    assert_eq!(global_dependencies.len(), 1);
    assert_eq!(
        global_dependencies[0].to(),
        fs::canonicalize(&global_helper).unwrap()
    );
    let global_cycle = index
        .dependency_cycle_for(&layer_conf, &global_base, WorkspaceClassContext::Global)
        .expect("the global inheritance chain should return to layer.conf");
    assert_eq!(global_cycle.first(), global_cycle.last());
    assert_eq!(global_cycle.len(), 4);

    let recipe_dependencies = index.dependencies_from(&recipe_base);
    assert_eq!(recipe_dependencies.len(), 1);
    assert_eq!(
        recipe_dependencies[0].to(),
        fs::canonicalize(&recipe_helper).unwrap()
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

fn write_path(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

impl Drop for TemporaryLayer {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(Path::new(&self.root));
    }
}
