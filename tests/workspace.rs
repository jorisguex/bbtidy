use bbtidy::WorkspaceIndex;
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
fn incomplete_file_contexts_do_not_appear_complete() {
    let layer = TemporaryLayer::new("workspace-incomplete");
    let recipe = layer.write("recipes-example/example.bb", "inherit base\n");
    let index = WorkspaceIndex::from_paths([recipe.clone()]).unwrap();

    assert!(!index.is_complete_for(&recipe));
    assert!(index.resolve_class("base").is_none());
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
