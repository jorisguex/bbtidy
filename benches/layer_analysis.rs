use bbtidy::{
    FormatOptions, LintOptions, WorkspaceIndex, format_with_options, lint_with_workspace,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ITERATIONS: u32 = 5;
const RECIPE_COUNT: usize = 1_000;

fn main() {
    let fixture = Fixture::new();
    let index_time = average_duration(ITERATIONS, || {
        let index = WorkspaceIndex::from_paths(&fixture.paths).expect("benchmark fixture paths");
        std::hint::black_box(index);
    });
    let format_time = average_duration(ITERATIONS, || {
        let formatted = format_with_options(Fixture::SOURCE, &FormatOptions::default())
            .expect("benchmark source should format");
        std::hint::black_box(formatted);
    });
    let workspace = WorkspaceIndex::from_paths(&fixture.paths).expect("benchmark fixture paths");
    let lint_time = average_duration(ITERATIONS, || {
        let mut finding_count = 0;
        for path in &fixture.recipe_paths {
            finding_count +=
                lint_with_workspace(Fixture::SOURCE, path, &workspace, &LintOptions::default())
                    .expect("benchmark source should lint")
                    .len();
        }
        std::hint::black_box(finding_count);
    });

    println!("bbtidy layer-analysis benchmark");
    println!("fixture files: {}", fixture.paths.len());
    println!("recipes linted per sample: {}", fixture.recipe_paths.len());
    print_duration("workspace index", index_time);
    print_duration("single-file format", format_time);
    print_duration("workspace lint batch", lint_time);
}

fn average_duration(iterations: u32, mut operation: impl FnMut()) -> Duration {
    let mut total = Duration::ZERO;
    for _ in 0..iterations {
        let start = Instant::now();
        operation();
        total += start.elapsed();
    }
    total / iterations
}

fn print_duration(name: &str, duration: Duration) {
    println!(
        "{name:24} {:>10.3} ms/sample",
        duration.as_secs_f64() * 1_000.0
    );
}

struct Fixture {
    root: PathBuf,
    paths: Vec<PathBuf>,
    recipe_paths: Vec<PathBuf>,
}

impl Fixture {
    const SOURCE: &'static str = "SUMMARY=\"benchmark\"\nrequire common.inc\ninherit base\n";

    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bbtidy-layer-analysis-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create benchmark fixture root");

        let mut paths = Vec::with_capacity(RECIPE_COUNT + 3);
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

        let mut recipe_paths = Vec::with_capacity(RECIPE_COUNT);
        for index in 0..RECIPE_COUNT {
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
