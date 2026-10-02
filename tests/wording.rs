//! The public crate says nothing about money, in strings, docs or comments,
//! and carries no machine-local paths.

use std::path::{Path, PathBuf};

const WHOLE_WORDS: [&str; 6] = ["plan", "tier", "credit", "trial", "paid", "free"];
const SUBSTRINGS: [&str; 6] = [
    "price",
    "billing",
    "subscription",
    "upgrade",
    "student",
    "$",
];

fn files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rs" | "md" | "json")
            ) {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = vec![root.join("README.md")];
    walk(&root.join("src"), &mut out);
    out.push(root.join("assets/suite.json"));
    out
}

#[test]
fn no_money_wording_in_src_or_readme() {
    for path in files() {
        let text = std::fs::read_to_string(&path).unwrap().to_lowercase();
        for word in SUBSTRINGS {
            assert!(!text.contains(word), "{word:?} in {}", path.display());
        }
        for word in text.split(|c: char| !c.is_ascii_alphanumeric()) {
            assert!(
                !WHOLE_WORDS.contains(&word),
                "{word:?} in {}",
                path.display()
            );
        }
    }
}

#[test]
fn no_local_paths_or_internal_names() {
    for path in files() {
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in ["/home/", "/Users/", "C:\\Users"] {
            assert!(!text.contains(needle), "{needle:?} in {}", path.display());
        }
    }
}
