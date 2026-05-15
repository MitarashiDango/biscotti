use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "Biscotti";
pub const CONFIG_FILE_NAME: &str = "config.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    pub config_version: u32,
    pub watch: WatchConfig,
    pub decode: DecodeConfig,
    pub ui: UiConfig,
    pub behavior: BehaviorConfig,
    #[serde(default)]
    pub history: HistoryConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryConfig {
    pub limit: HistoryLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HistoryLimit {
    Unlimited,
    Capped { value: u32 },
}

impl HistoryLimit {
    pub fn as_query_limit(self) -> u32 {
        match self {
            HistoryLimit::Unlimited => u32::MAX,
            HistoryLimit::Capped { value } => value,
        }
    }

    pub fn capped(self) -> Option<u32> {
        match self {
            HistoryLimit::Unlimited => None,
            HistoryLimit::Capped { value } => Some(value),
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            limit: HistoryLimit::Capped { value: 100 },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchConfig {
    pub folder_path: Option<PathBuf>,
    pub recursive: bool,
    pub mode: WatchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    Strict,
    Compatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeConfig {
    pub extensions: Vec<String>,
    pub file_ready_timeout_ms: u64,
    pub file_ready_check_interval_ms: u64,
    pub required_stable_checks: u32,
    #[serde(default)]
    pub preprocessing: DecodePreprocessingConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DecodePreprocessingConfig {
    pub force_contrast: bool,
    pub force_brighten: bool,
    pub force_threshold: bool,
    pub force_contrast_threshold: bool,
    pub force_invert: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: Theme,
    pub show_image_preview: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorConfig {
    pub auto_start_watch: bool,
    pub open_url_after_confirm: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            watch: WatchConfig {
                folder_path: None,
                recursive: true,
                mode: WatchMode::Strict,
            },
            decode: DecodeConfig {
                extensions: vec!["png".into(), "jpg".into(), "jpeg".into()],
                file_ready_timeout_ms: 10_000,
                file_ready_check_interval_ms: 400,
                required_stable_checks: 2,
                preprocessing: DecodePreprocessingConfig::default(),
            },
            ui: UiConfig {
                theme: Theme::System,
                show_image_preview: true,
            },
            behavior: BehaviorConfig {
                auto_start_watch: false,
                open_url_after_confirm: true,
            },
            history: HistoryConfig::default(),
        }
    }
}

pub fn app_data_dir() -> anyhow::Result<PathBuf> {
    let base_dirs =
        BaseDirs::new().ok_or_else(|| anyhow!("failed to resolve user base directories"))?;

    Ok(base_dirs.data_dir().join(APP_NAME))
}

pub fn default_config_path() -> anyhow::Result<PathBuf> {
    Ok(app_data_dir()?.join(CONFIG_FILE_NAME))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    CreatedDefault,
    RecoveredFromCorrupted { backup_path: PathBuf },
}

pub fn load_or_recover_default() -> anyhow::Result<(AppConfig, LoadOutcome)> {
    let path = default_config_path()?;
    load_or_recover_default_at(path)
}

pub fn load_or_recover_default_at(
    path: impl AsRef<Path>,
) -> anyhow::Result<(AppConfig, LoadOutcome)> {
    let path = path.as_ref();

    if !path.exists() {
        let config = AppConfig::default();
        save_config_atomic(path, &config)?;
        return Ok((config, LoadOutcome::CreatedDefault));
    }

    match load_config(path) {
        Ok(config) => Ok((config, LoadOutcome::Loaded)),
        Err(load_error) => {
            let backup_path = backup_path_for(path);
            if let Err(rename_err) = fs::rename(path, &backup_path) {
                return Err(load_error.context(format!(
                    "failed to back up corrupted config to {}: {rename_err}",
                    backup_path.display()
                )));
            }

            let config = AppConfig::default();
            save_config_atomic(path, &config)?;
            Ok((config, LoadOutcome::RecoveredFromCorrupted { backup_path }))
        }
    }
}

fn backup_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let backup_name = format!("{file_name}.broken.{timestamp}");
    path.with_file_name(backup_name)
}

pub fn load_config(path: impl AsRef<Path>) -> anyhow::Result<AppConfig> {
    let path = path.as_ref();
    let config_text = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    // TODO: handle config_version mismatches when the first migration is introduced.
    serde_json::from_str(&config_text)
        .with_context(|| format!("failed to parse config file: {}", path.display()))
}

pub fn save_config_atomic(path: impl AsRef<Path>, config: &AppConfig) -> anyhow::Result<()> {
    let path = path.as_ref();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }

    let temp_path = temp_path_for(path);
    let result = write_config_temp_file(&temp_path, config)
        .and_then(|_| replace_file(&temp_path, path).map_err(Into::into));

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn write_config_temp_file(path: &Path, config: &AppConfig) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create temporary config file: {}", path.display()))?;

    serde_json::to_writer_pretty(&mut file, config)
        .with_context(|| format!("failed to serialize config file: {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("failed to finalize config file: {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush config file: {}", path.display()))?;

    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(CONFIG_FILE_NAME);
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_file_name = format!(".{file_name}.{}.{}.tmp", std::process::id(), now_nanos);

    path.with_file_name(temp_file_name)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = wide_path(source);
    let destination = wide_path(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };

    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let config = AppConfig::default();

        assert_eq!(config.config_version, 1);
        assert_eq!(config.watch.folder_path, None);
        assert!(config.watch.recursive);
        assert_eq!(config.watch.mode, WatchMode::Strict);
        assert_eq!(config.decode.extensions, ["png", "jpg", "jpeg"]);
        assert_eq!(config.decode.file_ready_timeout_ms, 10_000);
        assert_eq!(config.decode.file_ready_check_interval_ms, 400);
        assert_eq!(config.decode.required_stable_checks, 2);
        assert_eq!(
            config.decode.preprocessing,
            DecodePreprocessingConfig::default()
        );
        assert_eq!(config.ui.theme, Theme::System);
        assert!(config.ui.show_image_preview);
        assert!(!config.behavior.auto_start_watch);
        assert!(config.behavior.open_url_after_confirm);
    }

    #[test]
    fn default_config_serializes_to_expected_json_shape() {
        let actual = serde_json::to_value(AppConfig::default()).expect("serialize default config");
        let expected = serde_json::json!({
            "config_version": 1,
            "watch": {
                "folder_path": null,
                "recursive": true,
                "mode": "strict",
            },
            "decode": {
                "extensions": ["png", "jpg", "jpeg"],
                "file_ready_timeout_ms": 10000,
                "file_ready_check_interval_ms": 400,
                "required_stable_checks": 2,
                "preprocessing": {
                    "force_contrast": false,
                    "force_brighten": false,
                    "force_threshold": false,
                    "force_contrast_threshold": false,
                    "force_invert": false,
                },
            },
            "ui": {
                "theme": "system",
                "show_image_preview": true,
            },
            "behavior": {
                "auto_start_watch": false,
                "open_url_after_confirm": true,
            },
            "history": {
                "limit": { "kind": "capped", "value": 100 },
            },
        });

        assert_eq!(actual, expected);
    }

    #[test]
    fn load_or_recover_default_recovers_from_corrupted_config() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join(CONFIG_FILE_NAME);
        fs::write(&path, "{ this is not valid json").expect("write corrupted config");

        let (config, outcome) =
            load_or_recover_default_at(&path).expect("recover from corrupted config");

        assert_eq!(config, AppConfig::default());
        let LoadOutcome::RecoveredFromCorrupted { backup_path } = outcome else {
            panic!("expected RecoveredFromCorrupted outcome, got {outcome:?}");
        };
        assert!(backup_path.exists(), "backup file should exist");
        assert!(
            backup_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{CONFIG_FILE_NAME}.broken."))),
            "backup file name should be {CONFIG_FILE_NAME}.broken.<timestamp>, got {}",
            backup_path.display()
        );
        assert_eq!(
            load_config(&path).expect("reload regenerated config"),
            AppConfig::default(),
            "config file should now contain default config"
        );
    }

    #[test]
    fn load_or_recover_default_marks_missing_config_as_created() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join(CONFIG_FILE_NAME);

        let (config, outcome) = load_or_recover_default_at(&path).expect("create default config");

        assert_eq!(config, AppConfig::default());
        assert_eq!(outcome, LoadOutcome::CreatedDefault);
    }

    #[test]
    fn load_or_recover_default_marks_existing_config_as_loaded() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join(CONFIG_FILE_NAME);
        save_config_atomic(&path, &AppConfig::default()).expect("save initial config");

        let (config, outcome) = load_or_recover_default_at(&path).expect("load existing config");

        assert_eq!(config, AppConfig::default());
        assert_eq!(outcome, LoadOutcome::Loaded);
    }

    #[test]
    fn load_config_defaults_missing_preprocessing_options() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join(CONFIG_FILE_NAME);
        fs::write(
            &path,
            r#"{
  "config_version": 1,
  "watch": {
    "folder_path": null,
    "recursive": true,
    "mode": "strict"
  },
  "decode": {
    "extensions": ["png", "jpg", "jpeg"],
    "file_ready_timeout_ms": 10000,
    "file_ready_check_interval_ms": 400,
    "required_stable_checks": 2
  },
  "ui": {
    "theme": "system",
    "show_image_preview": true
  },
  "behavior": {
    "auto_start_watch": false,
    "open_url_after_confirm": true
  }
}"#,
        )
        .expect("write old config");

        let config = load_config(&path).expect("load old config");

        assert_eq!(
            config.decode.preprocessing,
            DecodePreprocessingConfig::default()
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn save_config_atomic_replaces_existing_config() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join(CONFIG_FILE_NAME);

        save_config_atomic(&path, &AppConfig::default()).expect("save initial config");

        let mut changed = AppConfig::default();
        changed.config_version = 2;
        changed.watch.folder_path = Some(PathBuf::from(r"C:\Users\example\Pictures\Screenshots"));
        changed.ui.theme = Theme::Dark;

        save_config_atomic(&path, &changed).expect("replace config");

        assert_eq!(load_config(&path).expect("load replaced config"), changed);
    }

    #[test]
    fn load_config_rejects_invalid_json() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join(CONFIG_FILE_NAME);
        fs::write(&path, "{ invalid json").expect("write invalid config");

        let error = load_config(&path).expect_err("invalid config should fail");

        assert!(
            error.to_string().contains("failed to parse config file"),
            "{error:?}"
        );
    }
}
