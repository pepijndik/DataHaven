use crate::profile::BackupFilter;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Never worth backing up, whatever the profile says.
const ALWAYS_EXCLUDED: [&str; 9] = [
    ".DS_Store", "._*", ".Spotlight-V100", ".Trashes", ".fseventsd", ".TemporaryItems", "Backups.backupdb", "*.lbpart", "*.lbtmp-*",
];

/// Patterns without a `/` match the file or folder name anywhere (like .gitignore).
/// Patterns with a `/` match the path relative to the source root. Matching ignores case.
struct Matcher {
    names: GlobSet,
    paths: GlobSet,
}

impl Matcher {
    fn new<'a>(patterns: impl Iterator<Item = &'a str>) -> Result<Self, String> {
        let mut names = GlobSetBuilder::new();
        let mut paths = GlobSetBuilder::new();
        for pattern in patterns {
            let pattern = pattern.trim().trim_start_matches("./");
            if pattern.is_empty() {
                continue;
            }
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .case_insensitive(true)
                .build()
                .map_err(|error| format!("Invalid filter pattern \"{pattern}\": {error}"))?;
            if pattern.contains('/') { paths.add(glob); } else { names.add(glob); }
        }
        Ok(Self {
            names: names.build().map_err(|error| error.to_string())?,
            paths: paths.build().map_err(|error| error.to_string())?,
        })
    }

    fn is_match(&self, relative: &str) -> bool {
        let name = relative.rsplit('/').next().unwrap_or(relative);
        self.names.is_match(name) || self.paths.is_match(relative)
    }

    fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }
}

pub struct Filters {
    includes: Matcher,
    excludes: Matcher,
    /// Excludes such as `**/Previews.lrdata/**` turned into `**/Previews.lrdata`, so the walker skips the whole folder.
    pruned_directories: Matcher,
}

impl Filters {
    pub fn new(filters: &[BackupFilter]) -> Result<Self, String> {
        let patterns = |action: &'static str| filters.iter().filter(move |filter| filter.action == action).map(|filter| filter.pattern.as_str());
        let excludes: Vec<&str> = patterns("exclude").chain(ALWAYS_EXCLUDED).collect();
        let directory_patterns = excludes.iter().map(|pattern| pattern.strip_suffix("/**").unwrap_or(pattern));

        Ok(Self {
            includes: Matcher::new(patterns("include"))?,
            excludes: Matcher::new(excludes.iter().copied())?,
            pruned_directories: Matcher::new(directory_patterns)?,
        })
    }

    pub fn includes_file(&self, relative: &str) -> bool {
        (self.includes.is_empty() || self.includes.is_match(relative)) && !self.excludes.is_match(relative)
    }

    pub fn prunes_directory(&self, relative: &str) -> bool {
        self.pruned_directories.is_match(relative)
    }
}

/// Matches file names or relative paths against a list of patterns, used for "important" files.
pub struct PatternSet(Matcher);

impl PatternSet {
    pub fn new(patterns: &[String]) -> Result<Self, String> {
        Matcher::new(patterns.iter().map(String::as_str)).map(Self)
    }

    pub fn is_match(&self, relative: &str) -> bool {
        self.0.is_match(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(pattern: &str, action: &str) -> BackupFilter {
        BackupFilter { pattern: pattern.to_string(), action: action.to_string() }
    }

    #[test]
    fn excludes_generated_lightroom_folders_at_any_depth() {
        let filters = Filters::new(&[filter("**/*Previews.lrdata/**", "exclude")]).unwrap();
        assert!(filters.prunes_directory("Main Previews.lrdata"));
        assert!(filters.prunes_directory("Catalogs/Main Previews.lrdata"));
        assert!(filters.prunes_directory("Catalogs/Main Smart Previews.lrdata"));
        assert!(!filters.prunes_directory("Catalogs/Main.lrcat-data"));
        assert!(!filters.includes_file("Main Previews.lrdata/a/b.lrprev"));
        assert!(filters.includes_file("Catalogs/Main.lrcat"));
    }

    #[test]
    fn name_patterns_match_anywhere_and_ignore_case() {
        let filters = Filters::new(&[filter("*.tmp", "exclude")]).unwrap();
        assert!(!filters.includes_file("deep/folder/file.TMP"));
        assert!(filters.includes_file("deep/folder/file.jpg"));
    }

    #[test]
    fn includes_limit_files_but_never_prune_folders() {
        let filters = Filters::new(&[filter("*.mov", "include")]).unwrap();
        assert!(filters.includes_file("2024/clip.MOV"));
        assert!(!filters.includes_file("2024/photo.jpg"));
        assert!(!filters.prunes_directory("2024"));
    }

    #[test]
    fn always_skips_system_junk_and_partial_copies() {
        let filters = Filters::new(&[]).unwrap();
        assert!(!filters.includes_file("a/.DS_Store"));
        assert!(!filters.includes_file("a/._IMG_0001.CR3"));
        assert!(!filters.includes_file("a/IMG_0001.CR3.lbpart"));
        assert!(filters.prunes_directory(".Spotlight-V100"));
    }

    #[test]
    fn rejects_invalid_patterns() {
        assert!(Filters::new(&[filter("[unclosed", "exclude")]).is_err());
    }
}
