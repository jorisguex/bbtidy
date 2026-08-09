use bbtidy::{
    LintOptions, SemanticAnalysisOptions, SemanticDiagnosticPhase, SemanticDiagnosticStream,
    SemanticError, SemanticOptions, SemanticSeverity, analyze_bitbake, lint_with_bitbake,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
#[test]
fn authoritative_analysis_returns_resolved_environment_and_diagnostics() {
    let fixture = FakeBitBake::new(false);
    let options = SemanticOptions {
        bitbake: fixture.bitbake.clone(),
        build_dir: fixture.build_dir.clone(),
        targets: vec!["demo".to_owned()],
        variables: vec![
            "PN".to_owned(),
            "OVERRIDES".to_owned(),
            "MULTI".to_owned(),
            "MISSING".to_owned(),
        ],
        analysis: Default::default(),
    };

    let report = analyze_bitbake(&options).unwrap();

    assert_eq!(
        report.bitbake_version(),
        "BitBake Build Tool Core version 2.8.1"
    );
    assert!(report.parse_succeeded());
    assert_eq!(report.requested_targets(), &[String::from("demo")]);
    assert_eq!(
        report.requested_variables(),
        &[
            String::from("PN"),
            String::from("OVERRIDES"),
            String::from("MULTI"),
            String::from("MISSING"),
        ]
    );
    assert_eq!(report.environments().len(), 1);
    assert_eq!(report.target_results().len(), 1);
    assert!(report.target_results()[0].queried());
    assert!(report.target_results()[0].succeeded());
    let environment = &report.environments()[0];
    assert_eq!(environment.target(), "demo");
    assert_eq!(environment.get("PN"), Some("demo"));
    assert_eq!(environment.get("OVERRIDES"), Some("machine:class-native"));
    assert_eq!(environment.get("MULTI"), Some("one two"));
    assert_eq!(environment.get("MISSING"), None);
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.severity() == SemanticSeverity::Note)
    );
    let parse_note = report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.severity() == SemanticSeverity::Note)
        .unwrap();
    assert_eq!(parse_note.phase(), SemanticDiagnosticPhase::Parse);
    assert_eq!(parse_note.target(), None);
    assert_eq!(parse_note.stream(), SemanticDiagnosticStream::Stdout);
}

#[cfg(unix)]
#[test]
fn empty_variable_selection_keeps_raw_environment_without_materializing_values() {
    let fixture = FakeBitBake::new(false);
    let options = SemanticOptions {
        bitbake: fixture.bitbake.clone(),
        build_dir: fixture.build_dir.clone(),
        targets: vec!["demo".to_owned()],
        ..SemanticOptions::default()
    };

    let report = analyze_bitbake(&options).unwrap();
    let environment = &report.environments()[0];

    assert!(environment.variables().is_empty());
    assert!(environment.raw().contains("PN=\"demo\""));
}

#[cfg(unix)]
#[test]
fn semantic_lint_api_converts_bitbake_results_into_rule_diagnostics() {
    let fixture = FakeBitBake::new(false);
    let options = SemanticOptions {
        bitbake: fixture.bitbake.clone(),
        build_dir: fixture.build_dir.clone(),
        targets: vec!["demo".to_owned()],
        variables: vec![
            "SUMMARY".to_owned(),
            "DESCRIPTION".to_owned(),
            "LICENSE".to_owned(),
            "SRCREV".to_owned(),
            "SRCPV".to_owned(),
            "SRC_URI".to_owned(),
        ],
        analysis: Default::default(),
    };

    let (report, findings) = lint_with_bitbake(&options, &LintOptions::default()).unwrap();

    assert!(report.analysis_succeeded());
    assert!(
        findings
            .iter()
            .any(|finding| finding.diagnostic.rule_id() == "BBT019")
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.diagnostic.rule_id() == "BBT011")
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.diagnostic.rule_id() == "BBT012")
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.diagnostic.rule_id() == "BBT013")
    );
}

#[cfg(unix)]
#[test]
fn parse_failures_include_bitbake_source_locations() {
    let fixture = FakeBitBake::new(true);
    let options = SemanticOptions {
        bitbake: fixture.bitbake.clone(),
        build_dir: fixture.build_dir.clone(),
        ..SemanticOptions::default()
    };

    let report = analyze_bitbake(&options).unwrap();

    assert!(!report.parse_succeeded());
    let diagnostic = report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.severity() == SemanticSeverity::Error)
        .unwrap();
    assert_eq!(
        diagnostic.path(),
        Some(Path::new("/layer/recipes-demo/demo.bb"))
    );
    assert_eq!(diagnostic.line(), Some(12));
    assert_eq!(diagnostic.column(), Some(7));
    assert!(diagnostic.message().contains("invalid override"));
}

#[cfg(unix)]
#[test]
fn target_query_failures_are_reported_separately_from_parse_failures() {
    let fixture = FakeBitBake::target_failure();
    let options = SemanticOptions {
        bitbake: fixture.bitbake.clone(),
        build_dir: fixture.build_dir.clone(),
        targets: vec!["missing-target".to_owned()],
        ..SemanticOptions::default()
    };

    let report = analyze_bitbake(&options).unwrap();

    assert!(report.parse_succeeded());
    assert!(!report.target_queries_succeeded());
    assert!(!report.analysis_succeeded());
    assert!(report.environments().is_empty());
    assert_eq!(report.target_results().len(), 1);
    assert!(report.target_results()[0].queried());
    assert!(!report.target_results()[0].succeeded());
    assert_eq!(report.target_results()[0].target(), "missing-target");
    let target_error = &report.target_results()[0].diagnostics()[0];
    assert_eq!(target_error.phase(), SemanticDiagnosticPhase::TargetQuery);
    assert_eq!(target_error.target(), Some("missing-target"));
    assert_eq!(target_error.stream(), SemanticDiagnosticStream::Stderr);
    assert!(report.has_errors());
}

#[test]
fn invalid_build_context_is_reported_before_invocation() {
    let error = analyze_bitbake(&SemanticOptions::for_build_dir("/does/not/exist")).unwrap_err();
    assert!(matches!(error, SemanticError::InvalidBuildDirectory { .. }));
}

#[cfg(unix)]
#[test]
fn full_analysis_collects_graph_plan_inventory_and_package_metadata() {
    let fixture = FakeBitBake::full_analysis();
    fs::write(
        fixture.build_dir.join("task-depends.dot"),
        "digraph existing {}\n",
    )
    .unwrap();
    let options = SemanticOptions {
        bitbake: fixture.bitbake.clone(),
        build_dir: fixture.build_dir.clone(),
        targets: vec!["demo".to_owned()],
        variables: vec!["PN".to_owned()],
        analysis: SemanticAnalysisOptions::full(),
    };

    let report = analyze_bitbake(&options).unwrap();
    assert!(report.analysis_succeeded());
    let analysis = report.build_analysis().unwrap();
    assert!(analysis.succeeded());

    let graph = &analysis.graphs()[0];
    assert_eq!(graph.task_edges()[0].from(), "demo:do_build");
    assert_eq!(graph.task_edges()[0].to(), "lib:do_build");
    assert_eq!(graph.recipe_edges().len(), 1);
    assert_eq!(graph.package_edges().len(), 1);
    assert_eq!(
        graph.build_list(),
        &[String::from("demo"), String::from("lib")]
    );
    assert!(
        graph
            .providers()
            .iter()
            .any(|provider| { provider.name() == "virtual/foo" && provider.recipe() == "lib" })
    );

    let dry_run = analysis.dry_run().unwrap();
    assert_eq!(dry_run.tasks().len(), 2);
    assert!(dry_run.tasks()[0].contains("do_fetch"));

    let inventory = analysis.inventory().unwrap();
    assert_eq!(inventory.recipes().len(), 2);
    assert_eq!(inventory.recipes()[0].recipe(), "demo");
    assert_eq!(inventory.recipes()[0].version(), "1.0");
    assert!(
        inventory
            .providers()
            .iter()
            .any(|provider| { provider.name() == "virtual/foo" && provider.recipe() == "lib" })
    );

    let packages = &analysis.packages()[0];
    assert_eq!(
        packages.packages(),
        &[String::from("demo"), String::from("demo-dev")]
    );
    assert_eq!(packages.build_dependencies(), &[String::from("lib")]);
    assert_eq!(
        packages.image_install(),
        &[String::from("demo"), String::from("lib")]
    );
    assert_eq!(
        packages.runtime_dependencies().get("demo"),
        Some(&vec![String::from("lib")])
    );
    assert_eq!(
        packages.image_fstypes(),
        &[String::from("wic"), String::from("ext4")]
    );
    assert_eq!(
        fs::read_to_string(fixture.build_dir.join("task-depends.dot")).unwrap(),
        "digraph existing {}\n"
    );
}

#[cfg(unix)]
struct FakeBitBake {
    root: PathBuf,
    build_dir: PathBuf,
    bitbake: PathBuf,
}

#[cfg(unix)]
impl FakeBitBake {
    fn new(parse_failure: bool) -> Self {
        Self::create(parse_failure, false, false)
    }

    fn target_failure() -> Self {
        Self::create(false, true, false)
    }

    fn full_analysis() -> Self {
        Self::create(false, false, true)
    }

    fn create(parse_failure: bool, target_failure: bool, full_analysis: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let root = loop {
            let candidate = std::env::temp_dir().join(format!(
                "bbtidy-semantic-test-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create semantic test fixture: {error}"),
            }
        };
        let build_dir = root.join("build");
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

        let bitbake = root.join("bitbake");
        let script = if full_analysis {
            r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  echo 'NOTE: Parsing recipes'
  exit 0
fi
if [ "$1" = "--environment" ]; then
  printf 'PN="demo"\nPACKAGES="demo demo-dev"\nPROVIDES="demo virtual/foo"\nDEPENDS="lib"\nIMAGE_INSTALL="demo lib"\nIMAGE_FSTYPES="wic ext4"\nRDEPENDS:demo="lib"\nRRECOMMENDS:demo="optional"\nRPROVIDES:demo="demo-virtual"\n'
  exit 0
fi
if [ "$1" = "--show-versions" ]; then
  printf 'Recipe Name:Recipe Version:Preferred Provider\ndemo:1.0:demo\nlib:2.0:lib\n===============================\n'
  exit 0
fi
if [ "$1" = "--graphviz" ]; then
  printf 'digraph depends {\n  "demo:do_build" -> "lib:do_build";\n}\n' > task-depends.dot
  printf 'digraph depends {\n  "demo" -> "lib";\n}\n' > pn-depends.dot
  printf 'digraph depends {\n  "demo" -> "lib";\n}\n' > package-depends.dot
  printf 'demo\nlib\n' > pn-buildlist
  printf 'demo: demo\nlib: lib virtual/foo\n' > pn-provides
  exit 0
fi
if [ "$1" = "--dry-run" ]; then
  echo 'NOTE: Running task 1 of 2 (/layer/recipes-demo/demo.bb:do_fetch)'
  echo 'NOTE: Running task 2 of 2 (/layer/recipes-demo/demo.bb:do_build)'
  exit 0
fi
exit 0
"###
        } else if parse_failure {
            r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  echo 'ERROR: ParseError at /layer/recipes-demo/demo.bb:12:7: invalid override' >&2
  exit 1
fi
exit 0
"###
        } else if target_failure {
            r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  echo 'NOTE: Parsing recipes'
  exit 0
fi
if [ "$1" = "--environment" ]; then
  echo 'ERROR: Nothing PROVIDES missing-target' >&2
  exit 1
fi
exit 0
"###
        } else {
            r###"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'BitBake Build Tool Core version 2.8.1'
  exit 0
fi
if [ "$1" = "--parse-only" ]; then
  echo 'NOTE: Parsing recipes'
  exit 0
fi
if [ "$1" = "--environment" ]; then
  printf '# resolved by fake BitBake\nPN="demo"\nOVERRIDES="machine:class-native"\nMULTI="one \\\ntwo"\n'
  exit 0
fi
exit 0
"###
        };
        fs::write(&bitbake, script).unwrap();
        let mut permissions = fs::metadata(&bitbake).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bitbake, permissions).unwrap();

        Self {
            root,
            build_dir,
            bitbake,
        }
    }
}

#[cfg(unix)]
impl Drop for FakeBitBake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn unique_suffix() -> u64 {
    static NEXT_SUFFIX: AtomicU64 = AtomicU64::new(0);
    NEXT_SUFFIX.fetch_add(1, Ordering::Relaxed)
}
