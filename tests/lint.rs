use bbtidy::{
    LintFailurePolicy, LintFixError, LintOptions, LintSeverity, WorkspaceIndex, apply_lint_fixes,
    lint, lint_rules, lint_with_options, lint_with_workspace,
};
use std::fs;
use std::path::{Path, PathBuf};

const CORPUS_FILES: [&str; 6] = [
    "conf/layer.conf",
    "classes/example.bbclass",
    "recipes-example/example/example.inc",
    "recipes-example/example/example_1.0.bb",
    "recipes-example/example/example_%.bbappend",
    "recipes-example/example/compatibility.bb",
];

#[test]
fn public_lint_api_exposes_rule_metadata_and_diagnostics() {
    let rules = lint_rules();
    assert_eq!(rules.len(), 33);
    assert_eq!(rules[0].id(), "BBT001");
    assert_eq!(rules[0].name(), "trailing-whitespace");
    assert_eq!(rules[0].severity(), LintSeverity::Warning);
    assert!(!rules[0].description().is_empty());
    assert!(rules[0].fixable());
    assert!(!rules[3].fixable());

    let diagnostics = lint("SRCREV = \"${AUTOREV}\"\n").unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].rule_id(), "BBT004");
    assert_eq!(diagnostics[0].severity(), LintSeverity::Warning);
    assert_eq!(diagnostics[0].line(), 1);
    assert_eq!(diagnostics[0].column(), 11);
}

#[test]
fn public_diagnostics_expose_ranges_help_and_safe_fixes() {
    let source = "SUMMARY = \"demo\"  \nLICENSE = \"MIT\"";
    let diagnostics = lint(source).unwrap();

    assert_eq!(diagnostics[0].range().start(), 16);
    assert_eq!(diagnostics[0].range().end(), 18);
    assert_eq!(diagnostics[0].end_line(), 1);
    assert_eq!(diagnostics[0].end_column(), 19);
    assert!(diagnostics[0].is_fixable());
    assert_eq!(diagnostics[0].fixes()[0].replacement(), "");
    assert!(diagnostics[0].help().is_some());

    assert!(diagnostics[1].is_fixable());
    let fixed = apply_lint_fixes(source, &diagnostics).unwrap();
    assert_eq!(fixed, "SUMMARY = \"demo\"\nLICENSE = \"MIT\"\n");
    assert!(lint(&fixed).unwrap().is_empty());

    let autorev = lint("SRCREV = \"${AUTOREV}\"\n").unwrap();
    assert!(!autorev[0].is_fixable());
}

#[test]
fn applying_a_duplicate_edit_plan_fails_without_partial_output() {
    let source = "SUMMARY = \"demo\"  \n";
    let diagnostic = lint(source).unwrap().remove(0);
    let error = apply_lint_fixes(source, &[diagnostic.clone(), diagnostic]).unwrap_err();

    assert!(matches!(error, LintFixError::OverlappingRanges { .. }));
    assert_eq!(source, "SUMMARY = \"demo\"  \n");
}

#[test]
fn formatted_fixture_layer_is_clean_under_default_rules() {
    for relative_path in CORPUS_FILES {
        let path = corpus_root().join(relative_path);
        let text = fs::read_to_string(&path).unwrap();
        let diagnostics = lint(&text).unwrap();
        assert!(
            diagnostics.is_empty(),
            "{} produced unexpected diagnostics: {diagnostics:?}",
            path.display()
        );
    }
}

#[test]
fn public_lint_options_filter_rules_and_override_severity() {
    let mut options = LintOptions::default();
    options.disable_rule("BBT004");
    options.set_severity("BBT005", LintSeverity::Error);

    let diagnostics = lint_with_options(
        "SRCREV = \"${AUTOREV}\"\ninherit cmake\ninherit cmake\n",
        &options,
    )
    .unwrap();

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].rule_id(), "BBT005");
    assert_eq!(diagnostics[0].severity(), LintSeverity::Error);
}

#[test]
fn broader_lint_rules_cover_common_source_mistakes() {
    let source = concat!(
        "FILESEXTRAPATHS:prepend = \"${THISDIR}/files:\"\n",
        "SRC_URI = \"git://example.invalid/example.git;branch=main\"\n",
        "VALUE = \"one\"\n",
        "VALUE = \"two\"\n",
        "inherit\n",
        "do_build() {\n}\n",
        "do_build() {\n}\n",
    );
    let diagnostics = lint(source).unwrap();
    let ids = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.rule_id())
        .collect::<Vec<_>>();

    assert_eq!(ids, ["BBT014", "BBT015", "BBT016", "BBT018", "BBT017"]);
    assert_eq!((diagnostics[0].line(), diagnostics[0].column()), (1, 25));
    assert!(diagnostics[0].message().contains("path expansion"));
    assert!(
        diagnostics[1]
            .message()
            .contains("does not declare a protocol")
    );
    assert!(
        diagnostics[2]
            .message()
            .contains("assigned directly more than once")
    );
    assert!(
        diagnostics[3]
            .message()
            .contains("inherit directive has no target")
    );
    assert!(diagnostics[4].message().contains("declared more than once"));
}

#[test]
fn broader_recipe_qa_rules_cover_identity_sources_packages_and_uri_parameters() {
    let layer = TemporaryLayer::new("lint-recipe-qa");
    let configuration = layer.write(
        "conf/layer.conf",
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "BBFILE_COLLECTIONS += \"test\"\n",
            "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_test = \"1\"\n",
            "LAYERSERIES_COMPAT_test = \"test\"\n",
        ),
    );
    let source = concat!(
        "PN = \"other\"\n",
        "PV = \"2.0\"\n",
        "LICENSE = \"MIT\"\n",
        "LIC_FILES_CHKSUM = \"file://LICENSE\"\n",
        "SRC_URI = \"https://example.invalid/source.tar.gz;branch=main git://example.invalid/source.git;protocol=https;protocol=https\"\n",
        "PACKAGECONFIG = \"defined missing\"\n",
        "PACKAGECONFIG[defined] = \"--enable-defined\"\n",
        "PACKAGES = \"${PN} pkg pkg\"\n",
        "FILES:missing = \"/missing\"\n",
    );
    let recipe = layer.write("recipes-example/wrong_1.0.bb", source);
    let index = WorkspaceIndex::from_paths([configuration, recipe.clone()]).unwrap();

    let diagnostics =
        lint_with_workspace(source, &recipe, &index, &LintOptions::default()).unwrap();
    let ids = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.rule_id())
        .collect::<Vec<_>>();

    for expected in [
        "BBT020", "BBT021", "BBT022", "BBT023", "BBT024", "BBT025", "BBT026", "BBT027", "BBT028",
    ] {
        assert!(ids.contains(&expected), "missing {expected} in {ids:?}");
    }
}

#[test]
fn broader_recipe_qa_accepts_valid_checksums_packageconfig_and_package_scope() {
    let layer = TemporaryLayer::new("lint-recipe-qa-clean");
    let configuration = layer.write(
        "conf/layer.conf",
        concat!(
            "BBPATH .= \":${LAYERDIR}\"\n",
            "BBFILE_COLLECTIONS += \"test\"\n",
            "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
            "BBFILE_PRIORITY_test = \"1\"\n",
            "LAYERSERIES_COMPAT_test = \"test\"\n",
        ),
    );
    let source = concat!(
        "SUMMARY = \"example\"\n",
        "DESCRIPTION = \"example\"\n",
        "LICENSE = \"MIT\"\n",
        "LIC_FILES_CHKSUM = \"file://LICENSE;md5=0123456789abcdef0123456789abcdef\"\n",
        "SRC_URI = \"https://example.invalid/source.tar.gz;sha256sum=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n",
        "PACKAGECONFIG = \"feature\"\n",
        "PACKAGECONFIG[feature] = \"--enable-feature,--disable-feature,feature-dependency\"\n",
        "PACKAGES = \"valid valid-dev\"\n",
        "FILES:valid-dev = \"/usr/include\"\n",
    );
    let recipe = layer.write("recipes-example/valid_1.0.bb", source);
    let index = WorkspaceIndex::from_paths([configuration, recipe.clone()]).unwrap();

    let diagnostics =
        lint_with_workspace(source, &recipe, &index, &LintOptions::default()).unwrap();
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics for valid recipe QA metadata: {diagnostics:?}"
    );
}

#[test]
fn layer_qa_rules_validate_collections_patterns_priorities_dependencies_and_compatibility() {
    let layer = TemporaryLayer::new("lint-layer-qa");
    let source = concat!(
        "BBPATH .= \":${LAYERDIR}\"\n",
        "BBFILE_COLLECTIONS = \"test test\"\n",
        "BBFILE_PATTERN_test = \"\"\n",
        "BBFILE_PRIORITY_test = \"not-an-integer\"\n",
        "LAYERDEPENDS_test = \"missing\"\n",
    );
    let configuration = layer.write("conf/layer.conf", source);
    let index = WorkspaceIndex::from_paths([configuration.clone()]).unwrap();

    let diagnostics =
        lint_with_workspace(source, &configuration, &index, &LintOptions::default()).unwrap();
    let ids = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.rule_id())
        .collect::<Vec<_>>();

    assert!(ids.contains(&"BBT029"));
    assert!(ids.contains(&"BBT030"));
    assert!(ids.contains(&"BBT031"));
    assert!(ids.contains(&"BBT032"));
    assert!(ids.contains(&"BBT033"));
}

#[test]
fn broader_lint_rules_require_metadata_for_complete_recipe_workspaces() {
    let layer = TemporaryLayer::new("lint-recipe-metadata");
    let configuration = layer.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
    let source = "SUMMARY = \"example\"\n";
    let recipe = layer.write("recipes-example/example.bb", source);
    let index = WorkspaceIndex::from_paths([configuration, recipe.clone()]).unwrap();

    let diagnostics =
        lint_with_workspace(source, &recipe, &index, &LintOptions::default()).unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.rule_id())
            .collect::<Vec<_>>(),
        ["BBT012", "BBT013"]
    );
    assert!(diagnostics[0].message().contains("DESCRIPTION"));
    assert!(diagnostics[1].message().contains("LICENSE"));

    let isolated = lint(source).unwrap();
    assert!(isolated.is_empty());
}

#[test]
fn lint_failure_policy_uses_effective_diagnostic_severity() {
    let warning_diagnostics = lint("SRCREV = \"${AUTOREV}\"\n").unwrap();
    let mut options = LintOptions::default();

    assert_eq!(options.fail_on(), LintFailurePolicy::Warning);
    assert!(options.has_blocking_findings(&warning_diagnostics));

    options.set_fail_on(LintFailurePolicy::Error);
    assert!(!options.has_blocking_findings(&warning_diagnostics));

    options.set_fail_on(LintFailurePolicy::Never);
    assert!(!options.has_blocking_findings(&warning_diagnostics));

    options.set_fail_on(LintFailurePolicy::Info);
    options.set_severity("BBT004", LintSeverity::Info);
    let info_diagnostics = lint_with_options("SRCREV = \"${AUTOREV}\"\n", &options).unwrap();
    assert_eq!(info_diagnostics[0].severity(), LintSeverity::Info);
    assert!(options.has_blocking_findings(&info_diagnostics));

    options.set_fail_on(LintFailurePolicy::Warning);
    assert!(!options.has_blocking_findings(&info_diagnostics));
}

#[test]
fn workspace_lint_reports_same_priority_ambiguities() {
    let first = TemporaryLayer::new("lint-ambiguity-first");
    let second = TemporaryLayer::new("lint-ambiguity-second");
    let consumer = TemporaryLayer::new("lint-ambiguity-consumer");
    let first_conf = first.write("conf/layer.conf", "BBFILE_PRIORITY_first = \"5\"\n");
    let second_conf = second.write("conf/layer.conf", "BBFILE_PRIORITY_second = \"5\"\n");
    let consumer_conf = consumer.write("conf/layer.conf", "BBFILE_PRIORITY_consumer = \"1\"\n");
    let first_class = first.write("classes/base.bbclass", "BASE = \"first\"\n");
    let second_class = second.write("classes/base.bbclass", "BASE = \"second\"\n");
    let first_include = first.write("common.inc", "COMMON = \"first\"\n");
    let second_include = second.write("common.inc", "COMMON = \"second\"\n");
    let recipe = consumer.write(
        "recipes-example/example.bb",
        "include missing.inc\ninclude_all common.inc\nrequire common.inc\ninherit base\nSUMMARY = \"example\"\nDESCRIPTION = \"example\"\nLICENSE = \"CLOSED\"\n",
    );
    let index = WorkspaceIndex::from_paths([
        first_conf,
        second_conf,
        consumer_conf,
        first_class,
        second_class,
        first_include,
        second_include,
        recipe.clone(),
    ])
    .unwrap();

    let diagnostics = lint_with_workspace(
        "include missing.inc\ninclude_all common.inc\nrequire common.inc\ninherit base\nSUMMARY = \"example\"\nDESCRIPTION = \"example\"\nLICENSE = \"CLOSED\"\n",
        &recipe,
        &index,
        &LintOptions::default(),
    )
    .unwrap();

    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.rule_id())
            .collect::<Vec<_>>(),
        ["BBT008", "BBT009"]
    );
    assert!(diagnostics[0].message().contains("priority 5"));
    assert!(diagnostics[1].message().contains("priority 5"));
    assert!(diagnostics[0].message().contains("resolves to"));
    assert!(diagnostics[0].message().contains("BBPATH"));
}

#[test]
fn workspace_lint_reports_static_cycles_and_skips_dynamic_references() {
    let layer = TemporaryLayer::new("lint-dependency-cycle");
    let configuration = layer.write("conf/layer.conf", "BBPATH .= \":${LAYERDIR}\"\n");
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
    let source = concat!(
        "include helper.inc\n",
        "include_all shared.inc\n",
        "require required.inc\n",
        "inherit base\n",
        "include ${DYNAMIC}\n",
        "SUMMARY = \"example\"\n",
        "DESCRIPTION = \"example\"\n",
        "LICENSE = \"CLOSED\"\n",
    );
    let recipe = layer.write("recipes-example/example/example.bb", source);
    let index = WorkspaceIndex::from_paths([
        configuration,
        class,
        helper,
        required,
        shared,
        recipe.clone(),
    ])
    .unwrap();

    let diagnostics =
        lint_with_workspace(source, &recipe, &index, &LintOptions::default()).unwrap();

    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.rule_id())
            .collect::<Vec<_>>(),
        ["BBT010", "BBT010", "BBT010", "BBT010"]
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.line(), diagnostic.column()))
            .collect::<Vec<_>>(),
        [(1, 9), (2, 13), (3, 9), (4, 9)]
    );
    assert!(diagnostics[0].message().contains("current file directory"));
    assert!(diagnostics[1].message().contains("BBPATH"));
    assert!(diagnostics[3].message().contains("classes on BBPATH"));
}

#[test]
fn workspace_lint_uses_global_and_recipe_class_namespaces() {
    let layer = TemporaryLayer::new("lint-global-class-scope");
    let configuration_source = concat!(
        "BBPATH .= \":${LAYERDIR}\"\n",
        "BBFILE_COLLECTIONS += \"test\"\n",
        "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
        "BBFILE_PRIORITY_test = \"1\"\n",
        "LAYERSERIES_COMPAT_test = \"test\"\n",
        "INHERIT += \"global-base missing-global ${DYNAMIC}\"\n",
        "INHERIT:remove = \"missing-removed\"\n",
        "USER_CLASSES += \"metrics\"\n",
    );
    let configuration = layer.write("conf/layer.conf", configuration_source);
    let global_base = layer.write(
        "classes-global/global-base.bbclass",
        "inherit global-helper recipe-only\n",
    );
    let global_helper = layer.write(
        "classes-global/global-helper.bbclass",
        "ORIGIN = \"global\"\n",
    );
    let global_only = layer.write(
        "classes-global/global-only.bbclass",
        "ORIGIN = \"global\"\n",
    );
    let recipe_helper = layer.write(
        "classes-recipe/recipe-helper.bbclass",
        "ORIGIN = \"recipe\"\n",
    );
    let recipe_only = layer.write(
        "classes-recipe/recipe-only.bbclass",
        "ORIGIN = \"recipe\"\n",
    );
    let metrics = layer.write("classes/metrics.bbclass", "ORIGIN = \"shared\"\n");
    let recipe_source = concat!(
        "inherit recipe-helper global-only\n",
        "SUMMARY = \"example\"\n",
        "DESCRIPTION = \"example\"\n",
        "LICENSE = \"CLOSED\"\n",
    );
    let recipe = layer.write("recipes-example/example.bb", recipe_source);
    let index = WorkspaceIndex::from_paths([
        configuration.clone(),
        global_base.clone(),
        global_helper,
        global_only,
        recipe_helper,
        recipe_only,
        metrics,
        recipe.clone(),
    ])
    .unwrap();

    let configuration_diagnostics = lint_with_workspace(
        configuration_source,
        &configuration,
        &index,
        &LintOptions::default(),
    )
    .unwrap();
    assert_eq!(configuration_diagnostics.len(), 1);
    assert_eq!(configuration_diagnostics[0].rule_id(), "BBT007");
    assert!(
        configuration_diagnostics[0]
            .message()
            .contains("missing-global")
    );

    let global_diagnostics = lint_with_workspace(
        "inherit global-helper recipe-only\n",
        &global_base,
        &index,
        &LintOptions::default(),
    )
    .unwrap();
    assert_eq!(global_diagnostics.len(), 1);
    assert_eq!(global_diagnostics[0].rule_id(), "BBT007");
    assert!(global_diagnostics[0].message().contains("recipe-only"));

    let recipe_diagnostics =
        lint_with_workspace(recipe_source, &recipe, &index, &LintOptions::default()).unwrap();
    assert_eq!(recipe_diagnostics.len(), 1);
    assert_eq!(recipe_diagnostics[0].rule_id(), "BBT007");
    assert!(recipe_diagnostics[0].message().contains("global-only"));
}

#[test]
fn workspace_lint_reports_global_inherit_cycles() {
    let layer = TemporaryLayer::new("lint-global-inherit-cycle");
    let configuration_source = concat!(
        "BBPATH .= \":${LAYERDIR}\"\n",
        "BBFILE_COLLECTIONS += \"test\"\n",
        "BBFILE_PATTERN_test = \"^${LAYERDIR}/\"\n",
        "BBFILE_PRIORITY_test = \"1\"\n",
        "LAYERSERIES_COMPAT_test = \"test\"\n",
        "INHERIT += \"global-base\"\n",
    );
    let configuration = layer.write("conf/layer.conf", configuration_source);
    let global_base = layer.write(
        "classes-global/global-base.bbclass",
        "require conf/layer.conf\n",
    );
    let index = WorkspaceIndex::from_paths([configuration.clone(), global_base]).unwrap();

    let diagnostics = lint_with_workspace(
        configuration_source,
        &configuration,
        &index,
        &LintOptions::default(),
    )
    .unwrap();

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].rule_id(), "BBT010");
    assert_eq!((diagnostics[0].line(), diagnostics[0].column()), (6, 13));
    assert!(
        diagnostics[0]
            .message()
            .contains("static INHERIT dependency")
    );
    assert!(
        diagnostics[0]
            .message()
            .contains("classes-global on BBPATH")
    );
}

struct TemporaryLayer {
    root: PathBuf,
}

impl TemporaryLayer {
    fn new(label: &str) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/expected")
}
