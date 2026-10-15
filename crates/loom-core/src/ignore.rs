//! Intentionally small v1 ignore language for untracked files.
//! This is not Git's complete gitignore grammar: no negation, escapes or
//! per-directory files. Unsupported syntax fails closed with a clear error.
use crate::{Error, Result};
use globset::{GlobBuilder, GlobMatcher};
use std::{fs, path::Path};

pub(crate) struct IgnoreRules {
    patterns: Vec<GlobMatcher>,
}

impl IgnoreRules {
    pub fn load(root: &Path) -> Result<Self> {
        let source = match fs::read_to_string(root.join(".loomignore")) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err.into()),
        };
        let mut patterns = Vec::new();
        for (i, line) in source.lines().enumerate() {
            let value = line.trim();
            if value.is_empty() || value.starts_with('#') { continue; }
            if value.starts_with('!') || value.contains('\\') {
                return Err(Error::Invalid(format!(
                    ".loomignore:{}: negation and backslash escapes are not supported",
                    i + 1
                )));
            }
            let anchored = value.starts_with('/');
            let normalized = value.trim_start_matches('/');
            if normalized.is_empty() { continue; }
            let directory = normalized.ends_with('/');
            let pattern = normalized.trim_end_matches('/');
            if pattern.is_empty() { continue; }
            // A rule without a slash is a basename rule at any depth.
            // A slash-containing rule is anchored to the repository root.
            let prefix = if !anchored && !pattern.contains('/') { "**/" } else { "" };
            let expanded = if directory {
                format!("{prefix}{pattern}/**")
            } else {
                format!("{prefix}{pattern}")
            };
            let glob = GlobBuilder::new(&expanded)
                .literal_separator(true)
                .build()
                .map_err(|err| Error::Invalid(format!(
                    ".loomignore:{}: invalid pattern {value:?}: {err}", i + 1
                )))?;
            patterns.push(glob.compile_matcher());
        }
        Ok(Self { patterns })
    }

    pub fn ignores(&self, path: &str) -> bool {
        // Rules must never hide their own definition.
        if path == ".loomignore" { return false; }
        self.patterns.iter().any(|rule| rule.is_match(path))
    }
}

#[cfg(test)]
mod tests {
    use super::IgnoreRules;
    use std::fs;

    #[test]
    fn basename_and_rooted_directory_rules() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".loomignore"), "*.log\n/build/\ncache/\n#comment\n").unwrap();
        let rules = IgnoreRules::load(tmp.path()).unwrap();
        assert!(rules.ignores("error.log"));
        assert!(rules.ignores("nested/error.log"));
        assert!(rules.ignores("build/output.bin"));
        assert!(!rules.ignores("nested/build/output.bin"));
        assert!(rules.ignores("nested/cache/a.tmp"));
        assert!(!rules.ignores(".loomignore"));
        assert!(!rules.ignores("keep.txt"));
    }

    #[test]
    fn unsupported_negations_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".loomignore"), "*.log\n!important.log\n").unwrap();
        assert!(IgnoreRules::load(tmp.path()).is_err());
    }
}
