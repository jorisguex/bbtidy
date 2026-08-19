use bbtidy::{
    FormatOptions, LintOptions, WorkspaceIndex, format_with_options, get_line_col, lint,
    lint_with_workspace, parse, resolve_overrides,
};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LINE_COUNTS: &[usize] = &[1_000, 2_000, 4_000];
const DIAGNOSTIC_COUNTS: &[usize] = &[500, 1_000, 2_000];
const OVERRIDE_CHAIN_LENGTHS: &[usize] = &[125, 250, 500];
const WORKSPACE_INDEX_COUNTS: &[usize] = &[100, 500, 2_000];
const WORKSPACE_LINT_COUNTS: &[usize] = &[100, 500, 1_000];
const FORMAT_BYTES: &[usize] = &[1_024, 64 * 1_024, 1_024 * 1_024];

fn line_column_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("source/line_columns");
    for &line_count in LINE_COUNTS {
        let source = assignment_source(line_count, false);
        let offsets = source
            .match_indices('\n')
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>();
        assert_eq!(offsets.len(), line_count);
        group.throughput(Throughput::Elements(line_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(line_count),
            &line_count,
            |bencher, _| {
                bencher.iter(|| {
                    let checksum = offsets.iter().fold(0usize, |checksum, &offset| {
                        let (line, column) = get_line_col(black_box(&source), black_box(offset));
                        checksum ^ line ^ column
                    });
                    black_box(checksum)
                });
            },
        );
    }
    group.finish();
}

fn diagnostic_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("lint/dense_diagnostics");
    for &line_count in DIAGNOSTIC_COUNTS {
        let source = assignment_source(line_count, true);
        let finding_count = lint(&source)
            .expect("diagnostic benchmark source should parse")
            .len();
        assert!(finding_count >= line_count);
        group.throughput(Throughput::Elements(line_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(line_count),
            &line_count,
            |bencher, _| {
                bencher.iter(|| {
                    let findings =
                        lint(black_box(&source)).expect("diagnostic benchmark source should lint");
                    black_box(findings)
                });
            },
        );
    }
    group.finish();
}

fn override_resolution_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("overrides/chained_static_environment");
    for &assignment_count in OVERRIDE_CHAIN_LENGTHS {
        let source = chained_override_source(assignment_count);
        let tree = parse(&source).expect("override benchmark source should parse");
        let resolution = resolve_overrides(&tree);
        assert_eq!(resolution.get("V0"), Some("x"));
        group.throughput(Throughput::Elements(assignment_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(assignment_count),
            &assignment_count,
            |bencher, _| {
                bencher.iter(|| black_box(resolve_overrides(black_box(&tree))));
            },
        );
    }
    group.finish();
}

fn workspace_index_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("workspace/index_shared_include");
    for &recipe_count in WORKSPACE_INDEX_COUNTS {
        let fixture = Fixture::new(recipe_count);
        let index = WorkspaceIndex::from_paths(&fixture.paths)
            .expect("workspace benchmark fixture should index");
        black_box(index);
        group.throughput(Throughput::Elements(recipe_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(recipe_count),
            &recipe_count,
            |bencher, _| {
                bencher.iter(|| {
                    let index = WorkspaceIndex::from_paths(black_box(&fixture.paths))
                        .expect("workspace benchmark fixture should index");
                    black_box(index)
                });
            },
        );
    }
    group.finish();
}

fn workspace_lint_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("workspace/lint_shared_include");
    for &recipe_count in WORKSPACE_LINT_COUNTS {
        let fixture = Fixture::new(recipe_count);
        let workspace = WorkspaceIndex::from_paths(&fixture.paths)
            .expect("workspace benchmark fixture should index");
        let options = LintOptions::default();
        group.throughput(Throughput::Elements(recipe_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(recipe_count),
            &recipe_count,
            |bencher, _| {
                bencher.iter(|| {
                    let finding_count = fixture
                        .recipe_paths
                        .iter()
                        .map(|path| {
                            lint_with_workspace(
                                black_box(Fixture::SOURCE),
                                black_box(path),
                                black_box(&workspace),
                                black_box(&options),
                            )
                            .expect("workspace benchmark source should lint")
                            .len()
                        })
                        .sum::<usize>();
                    black_box(finding_count)
                });
            },
        );
    }
    group.finish();
}

fn format_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("format/source_bytes");
    let options = FormatOptions::default();
    for &source_bytes in FORMAT_BYTES {
        let source = source_for_size(source_bytes);
        let formatted =
            format_with_options(&source, &options).expect("format benchmark source should parse");
        assert!(!formatted.is_empty());
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(source_bytes),
            &source_bytes,
            |bencher, _| {
                bencher.iter(|| {
                    let formatted = format_with_options(black_box(&source), black_box(&options))
                        .expect("format benchmark source should format");
                    black_box(formatted)
                });
            },
        );
    }
    group.finish();
}

fn assignment_source(line_count: usize, trailing_whitespace: bool) -> String {
    let suffix = if trailing_whitespace { " \n" } else { "\n" };
    let mut source = String::with_capacity(line_count * 18);
    for index in 0..line_count {
        source.push_str(&format!("V{index} = \"x\"{suffix}"));
    }
    source
}

fn chained_override_source(assignment_count: usize) -> String {
    assert!(assignment_count > 0);
    let mut source = String::with_capacity(assignment_count * 22);
    for index in 0..assignment_count - 1 {
        source.push_str(&format!("V{index} = \"${{V{}}}\"\n", index + 1));
    }
    source.push_str(&format!("V{} = \"x\"\n", assignment_count - 1));
    source.push_str("OVERRIDES = \"machine\"\n");
    source
}

fn source_for_size(size: usize) -> String {
    const HEADER: &str = "SUMMARY = \"benchmark recipe\"\nLICENSE = \"MIT\"\n";
    const FILLER: &str = "# deterministic benchmark payload\n";
    let mut source = String::with_capacity(size + FILLER.len());
    source.push_str(HEADER);
    while source.len() < size {
        source.push_str(FILLER);
    }
    source
}

struct Fixture {
    root: PathBuf,
    paths: Vec<PathBuf>,
    recipe_paths: Vec<PathBuf>,
}

impl Fixture {
    const SOURCE: &'static str = "SUMMARY = \"benchmark\"\nrequire common.inc\ninherit base\n";

    fn new(recipe_count: usize) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bbtidy-layer-analysis-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create benchmark fixture root");

        let mut paths = Vec::with_capacity(recipe_count + 3);
        paths.push(write(
            &root,
            "conf/layer.conf",
            concat!(
                "BBFILE_COLLECTIONS += \"benchmark\"\n",
                "BBFILE_PATTERN_benchmark = \"^${LAYERDIR}/\"\n",
                "BBFILE_PRIORITY_benchmark = \"5\"\n",
                "BBPATH .= \":${LAYERDIR}\"\n",
                "BBPATH .= \":${LAYERDIR}/recipes-example\"\n",
            ),
        ));
        paths.push(write(&root, "classes/base.bbclass", "BASE = \"1\"\n"));
        paths.push(write(
            &root,
            "recipes-example/common.inc",
            "COMMON = \"1\"\n",
        ));

        let mut recipe_paths = Vec::with_capacity(recipe_count);
        for index in 0..recipe_count {
            let relative = format!("recipes-example/generated/recipe-{index}.bb");
            let path = write(&root, &relative, Self::SOURCE);
            paths.push(path.clone());
            recipe_paths.push(path);
        }

        Self {
            root,
            paths,
            recipe_paths,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("benchmark file parent"))
        .expect("create benchmark file parent");
    fs::write(&path, contents).expect("write benchmark fixture file");
    path
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(1_500))
        .noise_threshold(0.03)
        .significance_level(0.05)
        .confidence_level(0.95);
    targets =
        line_column_scaling,
        diagnostic_scaling,
        override_resolution_scaling,
        workspace_index_scaling,
        workspace_lint_scaling,
        format_scaling
}
criterion_main!(benches);
