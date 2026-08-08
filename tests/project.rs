use bbtidy::{
    BuildContext, BuildContextDiscoveryOptions, BuildContextError, BuildContextSource,
    discover_build_context_with_options,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn discovers_conventional_build_from_nested_project_path() {
    let fixture = Fixture::new();
    let build_dir = fixture.build("build");
    let source_file = fixture.write("meta-layer/recipes/demo.bb", "SUMMARY = \"demo\"\n");

    let context =
        discover_build_context_with_options(&source_file, &BuildContextDiscoveryOptions::default())
            .unwrap();

    assert_eq!(context.build_dir(), fs::canonicalize(build_dir).unwrap());
    assert_eq!(
        context.project_dir(),
        fs::canonicalize(fixture.root()).unwrap()
    );
    assert_eq!(context.source(), BuildContextSource::Discovered);
}

#[test]
fn explicit_context_validates_and_infers_project_parent() {
    let fixture = Fixture::new();
    let build_dir = fixture.build("custom-build");

    let context = BuildContext::from_build_dir(&build_dir).unwrap();

    assert_eq!(context.build_dir(), fs::canonicalize(build_dir).unwrap());
    assert_eq!(
        context.project_dir(),
        fs::canonicalize(fixture.root()).unwrap()
    );
    assert_eq!(context.source(), BuildContextSource::Explicit);
}

#[test]
fn configured_context_has_precedence_over_environment_candidates() {
    let fixture = Fixture::new();
    let configured = fixture.build("configured");
    let bbtidy_environment = fixture.build("bbtidy-environment");
    let builddir_environment = fixture.build("builddir-environment");
    let options = BuildContextDiscoveryOptions {
        configured_build_dir: Some(configured.clone()),
        bbtidy_build_dir: Some(bbtidy_environment),
        build_dir_environment: Some(builddir_environment),
    };

    let context = discover_build_context_with_options(fixture.root(), &options).unwrap();

    assert_eq!(context.build_dir(), fs::canonicalize(configured).unwrap());
    assert_eq!(context.source(), BuildContextSource::Configured);
}

#[test]
fn environment_precedence_prefers_bbtidy_override_over_builddir() {
    let fixture = Fixture::new();
    let bbtidy_environment = fixture.build("bbtidy-environment");
    let builddir_environment = fixture.build("builddir-environment");
    let options = BuildContextDiscoveryOptions {
        configured_build_dir: None,
        bbtidy_build_dir: Some(bbtidy_environment.clone()),
        build_dir_environment: Some(builddir_environment),
    };

    let context = discover_build_context_with_options(fixture.root(), &options).unwrap();

    assert_eq!(
        context.build_dir(),
        fs::canonicalize(bbtidy_environment).unwrap()
    );
    assert_eq!(context.source(), BuildContextSource::BbtidyEnvironment);
}

#[test]
fn relative_environment_path_is_resolved_from_discovery_start() {
    let fixture = Fixture::new();
    let build_dir = fixture.build("build");
    let options = BuildContextDiscoveryOptions {
        build_dir_environment: Some(PathBuf::from("build")),
        ..BuildContextDiscoveryOptions::default()
    };

    let context = discover_build_context_with_options(fixture.root(), &options).unwrap();

    assert_eq!(context.build_dir(), fs::canonicalize(build_dir).unwrap());
    assert_eq!(context.source(), BuildContextSource::BuildDirEnvironment);
}

#[test]
fn multiple_build_variants_require_explicit_selection() {
    let fixture = Fixture::new();
    fixture.build("build-debug");
    fixture.build("build-release");

    let error = discover_build_context_with_options(
        fixture.root(),
        &BuildContextDiscoveryOptions::default(),
    )
    .unwrap_err();

    assert!(matches!(error, BuildContextError::Ambiguous { .. }));
    assert!(error.to_string().contains("--build-dir"));
}

#[test]
fn missing_context_is_reported_instead_of_guessed() {
    let fixture = Fixture::new();

    let error = discover_build_context_with_options(
        fixture.root(),
        &BuildContextDiscoveryOptions::default(),
    )
    .unwrap_err();

    assert!(matches!(error, BuildContextError::NotFound { .. }));
}

#[test]
fn invalid_configured_context_reports_missing_configuration() {
    let fixture = Fixture::new();
    let invalid = fixture.root().join("not-a-build");
    fs::create_dir_all(&invalid).unwrap();
    let options = BuildContextDiscoveryOptions {
        configured_build_dir: Some(invalid),
        ..BuildContextDiscoveryOptions::default()
    };

    let error = discover_build_context_with_options(fixture.root(), &options).unwrap_err();

    assert!(matches!(
        error,
        BuildContextError::InvalidBuildDirectory {
            source: BuildContextSource::Configured,
            ..
        }
    ));
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "bbtidy-project-test-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn build(&self, relative: &str) -> PathBuf {
        let build_dir = self.root.join(relative);
        fs::create_dir_all(build_dir.join("conf")).unwrap();
        fs::write(
            build_dir.join("conf/local.conf"),
            "MACHINE = \"qemux86-64\"\n",
        )
        .unwrap();
        fs::write(
            build_dir.join("conf/bblayers.conf"),
            "BBLAYERS = \"/layer\"\n",
        )
        .unwrap();
        build_dir
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn unique_suffix() -> u64 {
    static NEXT_SUFFIX: AtomicU64 = AtomicU64::new(0);
    NEXT_SUFFIX.fetch_add(1, Ordering::Relaxed)
}
