//! `metaconfig.toml` decides where the other configuration files are read
//! from. It is discovered in the project `.maki/` directory first, then in the
//! regular config search dirs, and the first one found wins.
//!
//! ```toml
//! [files]
//! "providers.toml" = "alt/providers.toml" # exact file: used in place of the search
//! "permissions.toml" = "alt"              # directory: `alt/permissions.toml` is
//!                                         # searched before the regular config dirs
//! ```
//!
//! A directory value stands in for a whole alternate config tree: the same
//! name inside it is searched before the regular config dirs. A value that
//! names an existing file replaces the search for that name entirely; a path
//! that does not exist yet counts as a directory, so a typo keeps the regular
//! config working instead of silently dropping it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing::warn;

use crate::paths;

const METACONFIG_FILE: &str = "metaconfig.toml";
const FILES_TABLE: &str = "files";
const PROJECT_DIR: &str = ".maki";

type Entries = HashMap<String, PathBuf>;

static METACONFIG: OnceLock<Option<Entries>> = OnceLock::new();

/// Candidate paths for config file `name`, in priority order: a metaconfig
/// redirect first, then the regular config search dirs. Readers take the
/// first existing candidate.
pub fn candidates(name: &str) -> Vec<PathBuf> {
    candidates_with(cached(), &paths::config_search_dirs(), name)
}

/// The `<dir>/<name>` a directory redirect contributes for `name`, for callers
/// that iterate directories instead of single files (command, theme scanning).
pub fn dir_override(name: &str) -> Option<PathBuf> {
    dir_override_with(cached(), name)
}

pub(crate) fn find(name: &str) -> Option<PathBuf> {
    candidates(name).into_iter().find(|path| path.exists())
}

fn cached() -> Option<&'static Entries> {
    METACONFIG
        .get_or_init(|| {
            let cwd = std::env::current_dir().ok();
            load(cwd.as_deref(), &paths::config_search_dirs())
        })
        .as_ref()
}

fn load(cwd: Option<&Path>, dirs: &[PathBuf]) -> Option<Entries> {
    let path = metaconfig_path(cwd, dirs)?;
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cannot read metaconfig.toml");
            return None;
        }
    };
    match parse(&content, path.parent().unwrap_or_else(|| Path::new(""))) {
        Ok(entries) if entries.is_empty() => None,
        Ok(entries) => Some(entries),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "invalid metaconfig.toml, ignoring");
            None
        }
    }
}

/// The project `.maki/metaconfig.toml` wins over the global config dirs.
fn metaconfig_path(cwd: Option<&Path>, dirs: &[PathBuf]) -> Option<PathBuf> {
    cwd.map(|dir| dir.join(PROJECT_DIR).join(METACONFIG_FILE))
        .filter(|path| path.is_file())
        .or_else(|| {
            dirs.iter()
                .map(|dir| dir.join(METACONFIG_FILE))
                .find(|path| path.is_file())
        })
}

fn parse(content: &str, base: &Path) -> Result<Entries, String> {
    let value: toml::Value = toml::from_str(content).map_err(|e| e.to_string())?;
    let Some(files) = value.get(FILES_TABLE).and_then(|v| v.as_table()) else {
        return Ok(Entries::new());
    };
    let mut entries = Entries::new();
    for (name, value) in files {
        let target = value
            .as_str()
            .ok_or_else(|| format!("[{FILES_TABLE}].{name} must be a string path"))?;
        let expanded = paths::expand_tilde(Path::new(target));
        let path = if expanded.is_absolute() {
            expanded
        } else {
            base.join(expanded)
        };
        entries.insert(name.clone(), path);
    }
    Ok(entries)
}

fn candidates_with(entries: Option<&Entries>, dirs: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let normal = || dirs.iter().map(|dir| dir.join(name));
    match entries.and_then(|entries| entries.get(name)) {
        // Only a file that exists redirects exclusively; anything else (a
        // directory, or a path that does not exist yet) searches the same name
        // inside it first and falls back to the regular config dirs.
        Some(target) if target.is_file() => vec![target.clone()],
        Some(target) => std::iter::once(target.join(name)).chain(normal()).collect(),
        None => normal().collect(),
    }
}

fn dir_override_with(entries: Option<&Entries>, name: &str) -> Option<PathBuf> {
    entries?
        .get(name)
        .filter(|target| !target.is_file())
        .map(|target| target.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "providers.toml";

    fn dir(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn parse_resolves_relative_paths_against_the_metaconfig_dir() {
        let entries = parse("[files]\n\"x.toml\" = \"alt/x.toml\"\n", Path::new("/cfg")).unwrap();
        assert_eq!(entries["x.toml"], dir("/cfg/alt/x.toml"));
    }

    #[test]
    fn parse_expands_tilde() {
        let entries = parse("[files]\n\"x.toml\" = \"~/x.toml\"\n", Path::new("/cfg")).unwrap();
        let expected = paths::home().map(|home| home.join("x.toml"));
        assert_eq!(entries["x.toml"], expected.unwrap());
    }

    #[test]
    fn parse_keeps_absolute_paths() {
        let entries = parse("[files]\n\"x.toml\" = \"/etc/x.toml\"\n", Path::new("/cfg")).unwrap();
        assert_eq!(entries["x.toml"], dir("/etc/x.toml"));
    }

    #[test]
    fn parse_rejects_non_string_paths() {
        let err = parse("[files]\n\"x.toml\" = 3\n", Path::new("/cfg")).unwrap_err();
        assert!(err.contains("must be a string path"), "{err}");
    }

    #[test]
    fn parse_without_files_table_is_empty() {
        let entries = parse("[other]\nkey = \"value\"\n", Path::new("/cfg")).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn project_metaconfig_beats_config_dirs() {
        let cwd = tempfile::TempDir::new().unwrap();
        let global = tempfile::TempDir::new().unwrap();
        let project = cwd.path().join(PROJECT_DIR).join(METACONFIG_FILE);
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&project, "").unwrap();

        assert_eq!(
            metaconfig_path(Some(cwd.path()), &[global.path().to_path_buf()]),
            Some(project),
            "the project file must win even when a global one exists"
        );
    }

    #[test]
    fn metaconfig_falls_back_to_config_dirs() {
        let global = tempfile::TempDir::new().unwrap();
        let path = global.path().join(METACONFIG_FILE);
        std::fs::write(&path, "").unwrap();

        assert_eq!(
            metaconfig_path(None, &[global.path().to_path_buf()]),
            Some(path)
        );
    }

    #[test]
    fn no_metaconfig_anywhere_is_none() {
        let cwd = tempfile::TempDir::new().unwrap();
        let dirs = vec![tempfile::TempDir::new().unwrap().path().to_path_buf()];
        assert_eq!(metaconfig_path(Some(cwd.path()), &dirs), None);
    }

    #[test]
    fn without_an_entry_candidates_are_the_search_dirs() {
        let dirs = vec![dir("/legacy"), dir("/cfg")];
        assert_eq!(
            candidates_with(None, &dirs, NAME),
            vec![dir("/legacy/providers.toml"), dir("/cfg/providers.toml")]
        );
    }

    #[test]
    fn a_directory_entry_searches_the_same_name_in_it_first() {
        let root = tempfile::TempDir::new().unwrap();
        let alt = root.path().join("alt");
        std::fs::create_dir(&alt).unwrap();
        let dirs = vec![dir("/legacy"), dir("/cfg")];
        let mut entries = Entries::new();
        entries.insert(NAME.into(), alt.clone());

        assert_eq!(
            candidates_with(Some(&entries), &dirs, NAME),
            vec![
                alt.join(NAME),
                dir("/legacy/providers.toml"),
                dir("/cfg/providers.toml"),
            ]
        );
    }

    #[test]
    fn a_missing_path_entry_still_falls_back_to_the_search_dirs() {
        let dirs = vec![dir("/cfg")];
        let mut entries = Entries::new();
        entries.insert(NAME.into(), dir("/does/not/exist"));

        assert_eq!(
            candidates_with(Some(&entries), &dirs, NAME),
            vec![
                dir("/does/not/exist/providers.toml"),
                dir("/cfg/providers.toml")
            ]
        );
    }

    #[test]
    fn a_file_entry_replaces_the_search() {
        let root = tempfile::TempDir::new().unwrap();
        let file = root.path().join("custom.toml");
        std::fs::write(&file, "").unwrap();
        let dirs = vec![dir("/legacy"), dir("/cfg")];
        let mut entries = Entries::new();
        entries.insert(NAME.into(), file);

        assert_eq!(
            candidates_with(Some(&entries), &dirs, NAME),
            vec![root.path().join("custom.toml")]
        );
    }

    #[test]
    fn dir_override_joins_the_name_inside_the_redirect() {
        let root = tempfile::TempDir::new().unwrap();
        let alt = root.path().join("alt");
        std::fs::create_dir(&alt).unwrap();
        let mut entries = Entries::new();
        entries.insert("commands".into(), alt.clone());

        assert_eq!(
            dir_override_with(Some(&entries), "commands"),
            Some(alt.join("commands"))
        );
        assert_eq!(
            dir_override_with(Some(&entries), NAME),
            None,
            "only directories redirect dirs"
        );
        assert_eq!(dir_override_with(None, "commands"), None);
    }
}
