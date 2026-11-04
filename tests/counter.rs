// Checks the counter, which skips most rules for each path, against testing
// every rule's regex against every file and directory.
use gh_codeowners::counter::{Rule, Rules, count};
use gh_codeowners::pattern::compile;

// mulberry32, so failures reproduce from the seed.
struct Random(u32);

impl Random {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.next() as usize % items.len()]
    }
}

const NAMES: [&str; 12] = [
    "a", "b", "docs", "Docs", "src", "lib", "x.md", "y.MD", "z.ts", "[id]", "é", "a.b.c",
];
const SEGMENTS: [&str; 17] = [
    "a", "b", "docs", "Docs", "src", "*", "**", "*.md", "*.ts", "x*", "?", "[id]", "\\[id\\]", "é",
    "\\*", "a.b.c", "lib",
];

fn random_pattern(random: &mut Random) -> String {
    let segments: Vec<&str> = (0..1 + random.next() % 3)
        .map(|_| random.pick(&SEGMENTS))
        .collect();
    let mut pattern = segments.join("/");
    if random.next().is_multiple_of(3) {
        pattern.insert(0, '/');
    }
    if random.next().is_multiple_of(4) {
        pattern.push('/');
    }
    if random.next().is_multiple_of(8) {
        pattern.insert(0, '!');
    }
    pattern
}

fn random_files(random: &mut Random) -> Vec<String> {
    let mut files: Vec<String> = (0..1 + random.next() % 40)
        .map(|_| {
            (0..1 + random.next() % 4)
                .map(|_| random.pick(&NAMES))
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    // Like git's index: sorted, unique, and no file is also a directory.
    files.sort();
    files.dedup();
    let all = files.clone();
    files.retain(|f| !all.iter().any(|g| g.starts_with(&format!("{f}/"))));
    files
}

/// For each rule, the files it owns and whether it matches any path.
fn count_naively(patterns: &[String], files: &[String]) -> Vec<(usize, bool)> {
    let regexes: Vec<_> = patterns.iter().map(|p| compile(p)).collect();
    let mut result = vec![(0, false); patterns.len()];
    for file in files {
        // The file and each directory containing it.
        let mut paths: Vec<String> = file
            .match_indices('/')
            .map(|(i, _)| file[..=i].to_string())
            .collect();
        paths.push(file.clone());
        let mut owner = None;
        for (i, regex) in regexes.iter().enumerate() {
            let Some(regex) = regex else { continue };
            if paths.iter().any(|p| regex.is_match(p.as_bytes())) {
                result[i].1 = true;
                owner = Some(i);
            }
        }
        if let Some(i) = owner {
            result[i].0 += 1;
        }
    }
    result
}

#[test]
fn agrees_with_testing_every_rule() {
    for seed in 0..3000 {
        let mut random = Random(seed);
        let patterns: Vec<String> = (0..1 + random.next() % 8)
            .map(|_| random_pattern(&mut random))
            .collect();
        let files = random_files(&mut random);
        let rules: Vec<Rule> = patterns
            .iter()
            .enumerate()
            .map(|(lineno, pattern)| Rule {
                pattern: pattern.clone(),
                lineno,
            })
            .collect();
        let rules = Rules::new(&rules);
        let expected = count_naively(&patterns, &files);
        let counts = count(&rules, &files);
        let actual: Vec<(usize, bool)> = counts
            .rules
            .iter()
            .map(|r| (r.owned, r.matches_any))
            .collect();
        assert_eq!(
            actual, expected,
            "seed {seed}\npatterns: {patterns:?}\nfiles: {files:?}"
        );
    }
}

#[test]
fn owners_of_agrees_with_counting() {
    for seed in 0..1000 {
        let mut random = Random(seed);
        let patterns: Vec<String> = (0..1 + random.next() % 8)
            .map(|_| random_pattern(&mut random))
            .collect();
        let files = random_files(&mut random);
        let rules: Vec<Rule> = patterns
            .iter()
            .enumerate()
            .map(|(lineno, pattern)| Rule {
                pattern: pattern.clone(),
                lineno,
            })
            .collect();
        let compiled = Rules::new(&rules);
        let owners = gh_codeowners::owners_of(&rules, &files);
        for (file, owner) in files.iter().zip(owners) {
            let counted = count(&compiled, &[file])
                .rules
                .iter()
                .position(|rule| rule.owned == 1);
            assert_eq!(
                owner, counted,
                "seed {seed}\npatterns: {patterns:?}\nfile: {file}"
            );
        }
    }
}
