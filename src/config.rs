use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub input: InputConfig,
    pub output: OutputConfig,
    #[serde(default)]
    pub naming: NamingConfig,
}

/// Input configuration - where to find Rust source files
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    /// Single directory to scan for Rust files. Kept for backward
    /// compatibility; mutually exclusive with `source_dirs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dir: Option<PathBuf>,
    /// Directories to scan for Rust files, e.g. the `src` dir of every crate
    /// in a Cargo workspace. Each directory is treated as its own crate root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_dirs: Vec<PathBuf>,
    /// Directories or files to exclude from scanning
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Use cargo expand to handle macro-generated types (slower but more complete)
    #[serde(default)]
    pub use_cargo_expand: bool,
    /// Cargo manifest path for cargo expand (defaults to the Cargo.toml
    /// found above the first source directory)
    #[serde(default)]
    pub cargo_manifest: Option<PathBuf>,
}

impl InputConfig {
    /// The directories to scan: `source_dirs`, or `[source_dir]` for
    /// single-directory configs. Errors when both or neither are set.
    pub fn source_roots(&self) -> Result<Vec<&Path>> {
        match (&self.source_dir, self.source_dirs.is_empty()) {
            (Some(_), false) => anyhow::bail!(
                "Both `source_dir` and `source_dirs` are set in [input]. \
                Use only one of them (`source_dirs` for multiple directories)."
            ),
            (Some(dir), true) => Ok(vec![dir.as_path()]),
            (None, false) => Ok(self.source_dirs.iter().map(PathBuf::as_path).collect()),
            (None, true) => anyhow::bail!(
                "No source directory configured. Set `source_dirs = [\"src-tauri/src\"]` \
                (or `source_dir = \"src-tauri/src\"`) in [input]."
            ),
        }
    }
}

/// Output configuration - where to write generated TypeScript files
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    /// Path for generated TypeScript types file
    pub types_file: PathBuf,
    /// Path for generated TypeScript commands file
    pub commands_file: PathBuf,
}

/// Naming configuration - prefixes and suffixes for generated code
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NamingConfig {
    /// Prefix for TypeScript type names
    #[serde(default)]
    pub type_prefix: String,
    /// Suffix for TypeScript type names
    #[serde(default)]
    pub type_suffix: String,
    /// Prefix for TypeScript function names
    #[serde(default)]
    pub function_prefix: String,
    /// Suffix for TypeScript function names
    #[serde(default)]
    pub function_suffix: String,
}

impl Config {
    /// Load configuration from a TOML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        config.validate()?;

        Ok(config)
    }

    /// Validate the configuration (pure validation, no side effects)
    fn validate(&self) -> Result<()> {
        let roots = self.input.source_roots()?;
        for dir in &roots {
            if !dir.exists() {
                anyhow::bail!("Source directory does not exist: {}", dir.display());
            }
        }

        // Overlapping directories would scan the same files twice and turn
        // every command in them into a "duplicate command name" error.
        // Compare canonical paths so `./src` and `src` count as the same.
        let canonical: Vec<PathBuf> = roots
            .iter()
            .map(|dir| fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()))
            .collect();
        for (i, a) in canonical.iter().enumerate() {
            for (j, b) in canonical.iter().enumerate() {
                if i != j && b.starts_with(a) {
                    anyhow::bail!(
                        "Source directories overlap: {} contains {}. \
                        List each directory only once and don't nest them.",
                        roots[i].display(),
                        roots[j].display()
                    );
                }
            }
        }
        Ok(())
    }

    /// Ensure output directories exist, creating them if necessary
    /// This should be called before writing output files.
    pub fn ensure_output_directories(&self) -> Result<()> {
        // Note: create_dir_all is idempotent and handles race conditions
        if let Some(parent) = self.output.types_file.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create output directory: {}", parent.display())
                })?;
            }
        }

        if let Some(parent) = self.output.commands_file.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create output directory: {}", parent.display())
                })?;
            }
        }

        Ok(())
    }

    /// Generate a default configuration
    pub fn default_config() -> Self {
        Config {
            input: InputConfig {
                source_dir: Some(PathBuf::from("src-tauri/src")),
                exclude: vec!["tests".to_string(), "target".to_string()],
                ..Default::default()
            },
            output: OutputConfig {
                types_file: PathBuf::from("src/generated/types.ts"),
                commands_file: PathBuf::from("src/generated/commands.ts"),
            },
            naming: NamingConfig::default(),
        }
    }

    /// Save configuration to a TOML file
    pub fn save(&self, path: &Path) -> Result<()> {
        let content =
            toml::to_string_pretty(self).with_context(|| "Failed to serialize configuration")?;

        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = Config::default_config();

        assert_eq!(
            config.input.source_dir,
            Some(PathBuf::from("src-tauri/src"))
        );
        assert!(config.input.exclude.contains(&"tests".to_string()));
        assert!(config.input.exclude.contains(&"target".to_string()));
        assert_eq!(
            config.output.types_file,
            PathBuf::from("src/generated/types.ts")
        );
        assert_eq!(
            config.output.commands_file,
            PathBuf::from("src/generated/commands.ts")
        );
        assert!(config.naming.type_prefix.is_empty());
        assert!(config.naming.type_suffix.is_empty());
    }

    #[test]
    fn test_load_valid_config() {
        let dir = tempdir().unwrap();
        let source_dir = dir.path().join("src");
        fs::create_dir_all(&source_dir).unwrap();

        let config_content = format!(
            r#"
[input]
source_dir = '{}'
exclude = ["tests"]

[output]
types_file = "types.ts"
commands_file = "commands.ts"
"#,
            source_dir.display()
        );

        let config_path = dir.path().join("config.toml");
        let mut file = fs::File::create(&config_path).unwrap();
        file.write_all(config_content.as_bytes()).unwrap();

        let config = Config::load(&config_path).unwrap();

        assert_eq!(config.input.source_dir, Some(source_dir.clone()));
        assert_eq!(config.input.source_roots().unwrap(), vec![source_dir]);
        assert_eq!(config.input.exclude, vec!["tests"]);
        assert_eq!(config.output.types_file, PathBuf::from("types.ts"));
    }

    #[test]
    fn test_load_config_with_naming() {
        let dir = tempdir().unwrap();
        let source_dir = dir.path().join("src");
        fs::create_dir_all(&source_dir).unwrap();

        let config_content = format!(
            r#"
[input]
source_dir = '{}'

[output]
types_file = "types.ts"
commands_file = "commands.ts"

[naming]
type_prefix = "I"
type_suffix = "DTO"
function_prefix = "api"
function_suffix = "Cmd"
"#,
            source_dir.display()
        );

        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, config_content).unwrap();

        let config = Config::load(&config_path).unwrap();

        assert_eq!(config.naming.type_prefix, "I");
        assert_eq!(config.naming.type_suffix, "DTO");
        assert_eq!(config.naming.function_prefix, "api");
        assert_eq!(config.naming.function_suffix, "Cmd");
    }

    #[test]
    fn test_load_config_missing_naming_uses_defaults() {
        let dir = tempdir().unwrap();
        let source_dir = dir.path().join("src");
        fs::create_dir_all(&source_dir).unwrap();

        let config_content = format!(
            r#"
[input]
source_dir = '{}'

[output]
types_file = "types.ts"
commands_file = "commands.ts"
"#,
            source_dir.display()
        );

        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, config_content).unwrap();

        let config = Config::load(&config_path).unwrap();

        assert!(config.naming.type_prefix.is_empty());
        assert!(config.naming.type_suffix.is_empty());
    }

    #[test]
    fn test_load_invalid_toml() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "this is not valid toml [[[").unwrap();

        let result = Config::load(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_rejects_unknown_fields() {
        // A typo like `source_directry` used to be silently accepted
        // (the actual `source_dir` would then fail with "source dir does
        // not exist"). With `deny_unknown_fields` we fail at parse time
        // with a message that names the bad key.
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
[input]
source_dir = "src"
source_directry = "src"

[output]
types_file = "types.ts"
commands_file = "commands.ts"
"#,
        )
        .unwrap();

        let err = Config::load(&config_path).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("source_directry") || msg.to_lowercase().contains("unknown"),
            "error must point at the unknown field, got:\n{msg}"
        );
    }

    #[test]
    fn test_load_rejects_unknown_top_level_section() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
[input]
source_dir = "src"

[output]
types_file = "types.ts"
commands_file = "commands.ts"

[notation]
oops = "typo"
"#,
        )
        .unwrap();

        let err = Config::load(&config_path).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("notation") || msg.to_lowercase().contains("unknown"),
            "error must point at the unknown section, got:\n{msg}"
        );
    }

    #[test]
    fn test_load_missing_file() {
        let path = PathBuf::from("/nonexistent/path/config.toml");
        let result = Config::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_missing_source_dir() {
        let dir = tempdir().unwrap();

        let config_content = r#"
[input]
source_dir = "/nonexistent/source/dir"

[output]
types_file = "types.ts"
commands_file = "commands.ts"
"#;

        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, config_content).unwrap();

        let result = Config::load(&config_path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Source directory does not exist"));
    }

    /// Write `input_section` as the `[input]` table of a config in `dir`
    /// and load it.
    fn load_with_input(dir: &Path, input_section: &str) -> Result<Config> {
        let config_path = dir.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "[input]\n{input_section}\n[output]\ntypes_file = \"types.ts\"\ncommands_file = \"commands.ts\"\n"
            ),
        )
        .unwrap();
        Config::load(&config_path)
    }

    #[test]
    fn test_load_source_dirs() {
        let dir = tempdir().unwrap();
        let app = dir.path().join("app/src");
        let core = dir.path().join("core/src");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&core).unwrap();

        let config = load_with_input(
            dir.path(),
            &format!("source_dirs = ['{}', '{}']", app.display(), core.display()),
        )
        .unwrap();

        assert_eq!(config.input.source_dir, None);
        assert_eq!(config.input.source_roots().unwrap(), vec![app, core]);
    }

    #[test]
    fn test_source_dir_and_source_dirs_are_mutually_exclusive() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();

        let err = load_with_input(
            dir.path(),
            &format!("source_dir = '{0}'\nsource_dirs = ['{0}']", src.display()),
        )
        .unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("Both `source_dir` and `source_dirs`"),
            "got:\n{msg}"
        );
    }

    #[test]
    fn test_missing_source_dir_and_source_dirs() {
        let dir = tempdir().unwrap();

        for input in ["", "source_dirs = []"] {
            let err = load_with_input(dir.path(), input).unwrap_err();
            let msg = format!("{:#}", err);
            assert!(
                msg.contains("No source directory configured"),
                "input {input:?}, got:\n{msg}"
            );
        }
    }

    #[test]
    fn test_source_dirs_reports_the_missing_entry() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let missing = dir.path().join("crates/missing/src");

        let err = load_with_input(
            dir.path(),
            &format!(
                "source_dirs = ['{}', '{}']",
                src.display(),
                missing.display()
            ),
        )
        .unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("Source directory does not exist"), "{msg}");
        assert!(msg.contains("missing"), "{msg}");
    }

    #[test]
    fn test_source_dirs_must_not_overlap() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("app/src");
        fs::create_dir_all(&src).unwrap();

        // Nested directories and the same directory listed twice would both
        // scan files twice.
        for dirs in [
            format!("['{}', '{}']", dir.path().display(), src.display()),
            format!("['{0}', '{0}']", src.display()),
        ] {
            let err = load_with_input(dir.path(), &format!("source_dirs = {dirs}")).unwrap_err();
            let msg = format!("{:#}", err);
            assert!(msg.contains("Source directories overlap"), "{msg}");
        }
    }

    #[test]
    fn test_save_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("output.toml");

        let config = Config {
            input: InputConfig {
                source_dir: Some(PathBuf::from("src")),
                exclude: vec!["tests".to_string()],
                ..Default::default()
            },
            output: OutputConfig {
                types_file: PathBuf::from("types.ts"),
                commands_file: PathBuf::from("commands.ts"),
            },
            naming: NamingConfig {
                type_prefix: "I".to_string(),
                type_suffix: "".to_string(),
                function_prefix: "".to_string(),
                function_suffix: "".to_string(),
            },
        };

        config.save(&config_path).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("source_dir = \"src\""));
        // An unset `source_dirs` must not leak into the file: writing
        // `source_dirs = []` next to `source_dir` would read back fine, but
        // it's noise for the user editing the generated config.
        assert!(!content.contains("source_dirs"));
        assert!(content.contains("types_file = \"types.ts\""));
        assert!(content.contains("type_prefix = \"I\""));
    }

    #[test]
    fn test_naming_config_default() {
        let naming = NamingConfig::default();

        assert!(naming.type_prefix.is_empty());
        assert!(naming.type_suffix.is_empty());
        assert!(naming.function_prefix.is_empty());
        assert!(naming.function_suffix.is_empty());
    }

    #[test]
    fn test_config_with_empty_exclude() {
        let dir = tempdir().unwrap();
        let source_dir = dir.path().join("src");
        fs::create_dir_all(&source_dir).unwrap();

        let config_content = format!(
            r#"
[input]
source_dir = '{}'

[output]
types_file = "types.ts"
commands_file = "commands.ts"
"#,
            source_dir.display()
        );

        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, config_content).unwrap();

        let config = Config::load(&config_path).unwrap();

        assert!(config.input.exclude.is_empty());
    }
}
