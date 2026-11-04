// Counts the same files repeatedly, for profiling:
// cargo run --release --example bench_count <root> <rounds>
use gh_codeowners::codeowners::parse_codeowners;
use gh_codeowners::counter::{Rule, Rules, count};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap());
    let rounds: usize = args.next().map_or(10, |n| n.parse().unwrap());
    let text = std::fs::read_to_string(root.join(".github/CODEOWNERS")).unwrap();
    let rules: Vec<Rule> = parse_codeowners(&text)
        .into_iter()
        .map(|r| Rule {
            pattern: r.pattern,
            lineno: r.lineno,
        })
        .collect();
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .unwrap();
    let mut paths: Vec<&[u8]> = output.stdout.split(|&b| b == 0).collect();
    paths.pop();
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
