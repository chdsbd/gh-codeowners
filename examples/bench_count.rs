// Counts the same files repeatedly, for profiling:
// cargo run --release --example bench_count <root> <rounds> [threads]
use gh_codeowners::codeowners::parse_codeowners;
use gh_codeowners::counter::{Rule, Rules, count};
use gh_codeowners::index::Index;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap());
    let rounds: usize = args.next().map_or(10, |n| n.parse().unwrap());
    let threads: usize = args.next().map_or(1, |n| n.parse().unwrap());
    let text = std::fs::read_to_string(root.join(".github/CODEOWNERS")).unwrap();
    let rules: Vec<Rule> = parse_codeowners(&text)
        .into_iter()
        .map(|r| Rule {
            pattern: r.pattern,
            lineno: r.lineno,
        })
        .collect();
    let index = Index::open(&root).unwrap();
    let mut batches = Vec::new();
    index
        .for_each_batch(16 * 1024, |b| batches.push(b))
        .unwrap();
    let paths: Vec<&[u8]> = batches.iter().flat_map(|b| b.paths(Some(&index))).collect();
    let compiled = Rules::new(&rules);
    for _ in 0..rounds {
        let t = Instant::now();
        let counts = count(&compiled, &paths, threads);
        println!(
            "{} files: {:.1} ms",
            counts.files,
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
