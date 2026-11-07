// Counts the same files repeatedly, for profiling:
// cargo run --release --example bench_count <root> <rounds>
use gh_codeowners::TrackedFiles;
use gh_codeowners::codeowners::parse_codeowners;
use gh_codeowners::counter::{Rules, count, rule_list};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap());
    let rounds: usize = args.next().map_or(10, |n| n.parse().unwrap());
    let text = std::fs::read_to_string(root.join(".github/CODEOWNERS")).unwrap();
    let rules = rule_list(&parse_codeowners(&text));
    let files = TrackedFiles::read(&root).unwrap();
    let paths = files.paths();
    let compiled = Rules::new(&rules);
    for _ in 0..rounds {
        let t = Instant::now();
        let counts = count(&compiled, &paths);
        println!(
            "{} files: {:.1} ms",
            counts.files,
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
