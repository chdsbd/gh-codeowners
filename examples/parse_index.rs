// Times parsing the index alone:
// cargo run --release --example parse_index <root> [threads]
use gh_codeowners::index::Index;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap();
    let threads: usize = args.next().map_or(1, |n| n.parse().unwrap());
    let t = Instant::now();
    let index = Index::open(root.as_ref()).unwrap();
    let n = AtomicUsize::new(0);
    let ok = index
        .for_each_batch_parallel(threads, 16 * 1024, |b| {
            n.fetch_add(b.len(), Ordering::Relaxed);
        })
        .unwrap();
    println!(
        "{} paths in {:.1} ms (checks passed: {ok})",
        n.into_inner(),
        t.elapsed().as_secs_f64() * 1000.0
    );
}
