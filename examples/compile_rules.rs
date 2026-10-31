// Times turning every rule into a regex, which most rules skip:
// cargo run --release --example compile_rules <CODEOWNERS>
use gh_codeowners::codeowners::parse_codeowners;
use gh_codeowners::pattern::{Matcher, compile};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let rules = parse_codeowners(&std::fs::read_to_string(path).unwrap());
    let t = Instant::now();
    let regexes = rules.iter().filter_map(|r| compile(&r.pattern)).count();
    println!(
        "{regexes} regexes: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    let t = Instant::now();
    let needed = rules
        .iter()
        .filter(|r| matches!(Matcher::new(&r.pattern), Matcher::Regex { .. }))
        .count();
    println!(
        "matchers ({needed} need a regex): {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
}
