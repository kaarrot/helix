use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::{document::Mode, theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use toml::{de::Error as TomlError, map::Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub editor: helix_view::editor::Config,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<HashMap<Mode, KeyTrie>>,
    pub editor: Option<toml::Value>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            editor: helix_view::editor::Config::default(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoadWarning {
    pub path: String,
    pub message: String,
}

impl ConfigLoadWarning {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }

    fn from_error(path: impl Into<String>, error: &TomlError) -> Self {
        Self::new(path, error.message())
    }
}

impl Default for ConfigLoadError {
    fn default() -> Self {
        ConfigLoadError::Error(IOError::new(std::io::ErrorKind::NotFound, "place holder"))
    }
}

impl Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::BadConfig(err) => err.fmt(f),
            ConfigLoadError::Error(err) => err.fmt(f),
        }
    }
}

impl Display for ConfigLoadWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "Ignoring config: {}", self.message)
        } else {
            write!(f, "Ignoring config key `{}`: {}", self.path, self.message)
        }
    }
}

impl Config {
    pub fn load(
        global: Result<String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<Config, ConfigLoadError> {
        Config::load_with_warnings(global, local).map(|(config, _)| config)
    }

    pub fn load_with_warnings(
        global: Result<String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<(Config, Vec<ConfigLoadWarning>), ConfigLoadError> {
        let mut warnings = Vec::new();
        let global_config = load_config_file(global, &mut warnings)?;
        let local_config = load_config_file(local, &mut warnings)?;

        let config = match (global_config, local_config) {
            (Ok(global), Ok(local)) => build_config(Some(global), Some(local), &mut warnings),
            // if any configs are invalid return that first
            (_, Err(ConfigLoadError::BadConfig(err)))
            | (Err(ConfigLoadError::BadConfig(err)), _) => {
                return Err(ConfigLoadError::BadConfig(err))
            }
            (Ok(config), Err(_)) | (Err(_), Ok(config)) => {
                build_config(Some(config), None, &mut warnings)
            }

            // these are just two io errors return the one for the global config
            (Err(err), Err(_)) => return Err(err),
        };

        Ok((config, warnings))
    }

    pub fn load_default() -> Result<Config, ConfigLoadError> {
        Config::load_default_with_warnings().map(|(config, _)| config)
    }

    pub fn load_default_with_warnings() -> Result<(Config, Vec<ConfigLoadWarning>), ConfigLoadError>
    {
        let global_config =
            fs::read_to_string(helix_loader::config_file()).map_err(ConfigLoadError::Error);
        let local_config = fs::read_to_string(helix_loader::workspace_config_file())
            .map_err(ConfigLoadError::Error);
        Config::load_with_warnings(global_config, local_config)
    }
}

fn load_config_file(
    file: Result<String, ConfigLoadError>,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> Result<Result<ConfigRaw, ConfigLoadError>, ConfigLoadError> {
    match file {
        Ok(file) => sanitize_config_file(&file, warnings).map(Ok),
        Err(err) => Ok(Err(err)),
    }
}

fn sanitize_config_file(
    file: &str,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> Result<ConfigRaw, ConfigLoadError> {
    let value: Value = toml::from_str(file).map_err(ConfigLoadError::BadConfig)?;
    let Value::Table(mut table) = value else {
        warnings.push(ConfigLoadWarning::new("", "expected a TOML table"));
        return Ok(ConfigRaw::default());
    };

    for key in table
        .keys()
        .filter(|key| !matches!(key.as_str(), "theme" | "keys" | "editor"))
        .cloned()
        .collect::<Vec<_>>()
    {
        table.remove(&key);
        warnings.push(ConfigLoadWarning::new(key, "unknown top-level key"));
    }

    let theme = table
        .remove("theme")
        .and_then(|value| sanitize_theme(value, warnings));
    let keys = table
        .remove("keys")
        .and_then(|value| sanitize_keys(value, warnings));
    let editor = table
        .remove("editor")
        .and_then(|value| sanitize_editor(value, warnings));

    Ok(ConfigRaw {
        theme,
        keys,
        editor,
    })
}

fn build_config(
    global: Option<ConfigRaw>,
    local: Option<ConfigRaw>,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> Config {
    let ConfigRaw {
        theme: global_theme,
        keys: global_keys,
        editor: global_editor,
    } = global.unwrap_or_default();
    let ConfigRaw {
        theme: local_theme,
        keys: local_keys,
        editor: local_editor,
    } = local.unwrap_or_default();

    let mut keys = keymap::default();
    if let Some(global_keys) = global_keys {
        merge_keys(&mut keys, global_keys);
    }
    if let Some(local_keys) = local_keys {
        merge_keys(&mut keys, local_keys);
    }

    let editor = match (global_editor, local_editor) {
        (None, None) => helix_view::editor::Config::default(),
        (None, Some(val)) | (Some(val), None) => editor_from_value(val, warnings),
        (Some(global), Some(local)) => {
            editor_from_value(merge_toml_values(global, local, 3), warnings)
        }
    };

    Config {
        theme: local_theme.or(global_theme),
        keys,
        editor,
    }
}

fn sanitize_theme(value: Value, warnings: &mut Vec<ConfigLoadWarning>) -> Option<theme::Config> {
    let theme: Result<theme::Config, TomlError> = value.try_into();
    match theme {
        Ok(theme) => Some(theme),
        Err(err) => {
            warnings.push(ConfigLoadWarning::from_error("theme", &err));
            None
        }
    }
}

fn editor_from_value(
    value: Value,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> helix_view::editor::Config {
    let Some(value) = sanitize_editor(value, warnings) else {
        return helix_view::editor::Config::default();
    };

    let editor: Result<helix_view::editor::Config, TomlError> = value.try_into();
    match editor {
        Ok(editor) => editor,
        Err(err) => {
            warnings.push(ConfigLoadWarning::from_error("editor", &err));
            helix_view::editor::Config::default()
        }
    }
}

fn sanitize_editor(value: Value, warnings: &mut Vec<ConfigLoadWarning>) -> Option<Value> {
    let root_error = match editor_value_is_valid(&[], &value) {
        Ok(()) => return Some(value),
        Err(err) => err,
    };

    match value {
        Value::Table(table) => {
            let sanitized = sanitize_editor_table(table, &mut Vec::new(), warnings);
            if sanitized.is_empty() {
                return None;
            }

            let value = Value::Table(sanitized);
            match editor_value_is_valid(&[], &value) {
                Ok(()) => Some(value),
                Err(err) => {
                    warnings.push(ConfigLoadWarning::from_error("editor", &err));
                    None
                }
            }
        }
        _ => {
            warnings.push(ConfigLoadWarning::from_error("editor", &root_error));
            None
        }
    }
}

fn sanitize_editor_table(
    table: Map<String, Value>,
    path: &mut Vec<String>,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> Map<String, Value> {
    let mut sanitized = Map::new();

    for (key, value) in table {
        path.push(key.clone());
        match editor_value_is_valid(path, &value) {
            Ok(()) => {
                sanitized.insert(key, value);
            }
            Err(err) => match value {
                Value::Table(table) => {
                    let warning_count = warnings.len();
                    let child = sanitize_editor_table(table, path, warnings);
                    if child.is_empty() {
                        if warnings.len() == warning_count {
                            warnings.push(ConfigLoadWarning::from_error(
                                config_path("editor", path),
                                &err,
                            ));
                        }
                    } else {
                        let value = Value::Table(child);
                        match editor_value_is_valid(path, &value) {
                            Ok(()) => {
                                sanitized.insert(key, value);
                            }
                            Err(err) => warnings.push(ConfigLoadWarning::from_error(
                                config_path("editor", path),
                                &err,
                            )),
                        }
                    }
                }
                _ => warnings.push(ConfigLoadWarning::from_error(
                    config_path("editor", path),
                    &err,
                )),
            },
        }
        path.pop();
    }

    sanitized
}

fn editor_value_is_valid(path: &[String], value: &Value) -> Result<(), TomlError> {
    let editor: Result<helix_view::editor::Config, TomlError> =
        nested_value(path, value.clone()).try_into();
    editor.map(|_| ())
}

fn sanitize_keys(
    value: Value,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> Option<HashMap<Mode, KeyTrie>> {
    let Value::Table(modes) = value else {
        warnings.push(ConfigLoadWarning::new("keys", "expected keymap table"));
        return None;
    };

    let mut sanitized_modes = Map::new();
    for (mode, value) in modes {
        if let Err(err) = key_mode_is_valid(&mode) {
            warnings.push(ConfigLoadWarning::from_error(format!("keys.{mode}"), &err));
            continue;
        }

        let Value::Table(table) = value else {
            warnings.push(ConfigLoadWarning::new(
                format!("keys.{mode}"),
                "expected keymap table",
            ));
            continue;
        };

        let value = Value::Table(table);
        match key_entry_is_valid(&mode, &[], &value) {
            Ok(()) => {
                sanitized_modes.insert(mode, value);
            }
            Err(_) => {
                let Value::Table(table) = value else {
                    unreachable!("key mode values were checked as tables");
                };
                let child = sanitize_key_table(&mode, table, &mut Vec::new(), warnings);
                if !child.is_empty() {
                    let value = Value::Table(child);
                    match key_entry_is_valid(&mode, &[], &value) {
                        Ok(()) => {
                            sanitized_modes.insert(mode, value);
                        }
                        Err(err) => warnings
                            .push(ConfigLoadWarning::from_error(format!("keys.{mode}"), &err)),
                    }
                }
            }
        }
    }

    if sanitized_modes.is_empty() {
        return None;
    }

    let keys: Result<HashMap<Mode, KeyTrie>, TomlError> = Value::Table(sanitized_modes).try_into();
    match keys {
        Ok(keys) => Some(keys),
        Err(err) => {
            warnings.push(ConfigLoadWarning::from_error("keys", &err));
            None
        }
    }
}

fn sanitize_key_table(
    mode: &str,
    table: Map<String, Value>,
    path: &mut Vec<String>,
    warnings: &mut Vec<ConfigLoadWarning>,
) -> Map<String, Value> {
    let mut sanitized = Map::new();

    for (key, value) in table {
        path.push(key.clone());
        if let Err(err) = key_path_accepts_key(mode, path) {
            warnings.push(ConfigLoadWarning::from_error(
                config_path(&format!("keys.{mode}"), path),
                &err,
            ));
            path.pop();
            continue;
        }

        match key_entry_is_valid(mode, path, &value) {
            Ok(()) => {
                sanitized.insert(key, value);
            }
            Err(err) => match value {
                Value::Table(table) => {
                    let warning_count = warnings.len();
                    let child = sanitize_key_table(mode, table, path, warnings);
                    if child.is_empty() {
                        if warnings.len() == warning_count {
                            warnings.push(ConfigLoadWarning::from_error(
                                config_path(&format!("keys.{mode}"), path),
                                &err,
                            ));
                        }
                    } else {
                        let value = Value::Table(child);
                        match key_entry_is_valid(mode, path, &value) {
                            Ok(()) => {
                                sanitized.insert(key, value);
                            }
                            Err(err) => warnings.push(ConfigLoadWarning::from_error(
                                config_path(&format!("keys.{mode}"), path),
                                &err,
                            )),
                        }
                    }
                }
                _ => warnings.push(ConfigLoadWarning::from_error(
                    config_path(&format!("keys.{mode}"), path),
                    &err,
                )),
            },
        }
        path.pop();
    }

    sanitized
}

fn key_mode_is_valid(mode: &str) -> Result<(), TomlError> {
    let mut modes = Map::new();
    modes.insert(mode.to_owned(), Value::Table(Map::new()));
    let keys: Result<HashMap<Mode, KeyTrie>, TomlError> = Value::Table(modes).try_into();
    keys.map(|_| ())
}

fn key_path_accepts_key(mode: &str, path: &[String]) -> Result<(), TomlError> {
    key_entry_is_valid(mode, path, &Value::String("no_op".to_owned()))
}

fn key_entry_is_valid(mode: &str, path: &[String], value: &Value) -> Result<(), TomlError> {
    let mut modes = Map::new();
    modes.insert(mode.to_owned(), nested_value(path, value.clone()));
    let keys: Result<HashMap<Mode, KeyTrie>, TomlError> = Value::Table(modes).try_into();
    keys.map(|_| ())
}

fn nested_value(path: &[String], leaf: Value) -> Value {
    let mut value = leaf;
    for key in path.iter().rev() {
        let mut table = Map::new();
        table.insert(key.clone(), value);
        value = Value::Table(table);
    }
    value
}

fn config_path(root: &str, path: &[String]) -> String {
    if path.is_empty() {
        root.to_owned()
    } else {
        format!("{root}.{}", path.join("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::document::Mode;

    impl Config {
        fn load_test(config: &str) -> Config {
            Config::load(Ok(config.to_owned()), Err(ConfigLoadError::default())).unwrap()
        }

        fn load_test_with_warnings(config: &str) -> (Config, Vec<ConfigLoadWarning>) {
            Config::load_with_warnings(Ok(config.to_owned()), Err(ConfigLoadError::default()))
                .unwrap()
        }
    }

    fn assert_cursorline_modes(config: &Config, normal: bool, insert: bool, select: bool) {
        assert_eq!(config.editor.cursorline.from_mode(Mode::Normal), normal);
        assert_eq!(config.editor.cursorline.from_mode(Mode::Insert), insert);
        assert_eq!(config.editor.cursorline.from_mode(Mode::Select), select);
    }

    #[test]
    fn parsing_keymaps_config_file() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let sample_keymaps = r#"
            [keys.insert]
            y = "move_line_down"
            S-C-a = "delete_selection"

            [keys.normal]
            A-F12 = "move_next_word_end"
        "#;

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Insert => keymap!({ "Insert mode"
                    "y" => move_line_down,
                    "S-C-a" => delete_selection,
                }),
                Mode::Normal => keymap!({ "Normal mode"
                    "A-F12" => move_next_word_end,
                }),
            },
        );

        assert_eq!(
            Config::load_test(sample_keymaps),
            Config {
                keys,
                ..Default::default()
            }
        );
    }

    #[test]
    fn keys_resolve_to_correct_defaults() {
        // From serde default
        let default_keys = Config::load_test("").keys;
        assert_eq!(default_keys, keymap::default());

        // From the Default trait
        let default_keys = Config::default().keys;
        assert_eq!(default_keys, keymap::default());
    }

    #[test]
    fn auto_reload_config_defaults_to_false_and_accepts_true() {
        assert!(!Config::load_test("").editor.auto_reload);
        assert!(
            Config::load_test("[editor]\nauto-reload = true")
                .editor
                .auto_reload
        );
    }

    #[test]
    fn unknown_top_level_keys_do_not_discard_valid_config() {
        let (config, warnings) = Config::load_test_with_warnings(
            r#"
            unknown = true

            [editor]
            scrolloff = 12
            "#,
        );

        assert_eq!(config.editor.scrolloff, 12);
        assert!(warnings.iter().any(|warning| warning.path == "unknown"));
    }

    #[test]
    fn invalid_editor_field_does_not_discard_valid_siblings() {
        let (config, warnings) = Config::load_test_with_warnings(
            r#"
            [editor]
            scrolloff = 9
            mouse = "no"
            cursorline = true
            "#,
        );

        assert_eq!(config.editor.scrolloff, 9);
        assert_cursorline_modes(&config, true, true, true);
        assert_eq!(
            config.editor.mouse,
            helix_view::editor::Config::default().mouse
        );
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "editor.mouse"));
    }

    #[test]
    fn invalid_nested_editor_field_does_not_discard_valid_siblings() {
        use helix_view::editor::StatusLineElement as E;

        let (config, warnings) = Config::load_test_with_warnings(
            r#"
            [editor.statusline]
            left = ["mode"]
            center = ["file-name"]
            right = 1
            unknown = "value"
            "#,
        );

        assert_eq!(config.editor.statusline.left, vec![E::Mode]);
        assert_eq!(config.editor.statusline.center, vec![E::FileName]);
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "editor.statusline.right"));
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "editor.statusline.unknown"));
    }

    #[test]
    fn invalid_keybinding_does_not_discard_other_keys_or_editor_config() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let (config, warnings) = Config::load_test_with_warnings(
            r#"
            [editor]
            scrolloff = 8

            [keys.normal]
            y = "move_line_down"
            S-C-a = "definitely_not_a_command"
            "#,
        );

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Normal => keymap!({ "Normal mode"
                    "y" => move_line_down,
                }),
            },
        );

        assert_eq!(config.editor.scrolloff, 8);
        assert_eq!(config.keys, keys);
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "keys.normal.S-C-a"));
    }

    #[test]
    fn global_and_workspace_configs_merge_after_recoverable_warnings() {
        let global = r#"
            unknown = true

            [editor]
            scrolloff = 7
            mouse = "bad"
        "#;
        let local = r#"
            [editor]
            cursorline = true
            scroll-lines = "bad"
        "#;

        let (config, warnings) =
            Config::load_with_warnings(Ok(global.to_owned()), Ok(local.to_owned())).unwrap();

        assert_eq!(config.editor.scrolloff, 7);
        assert_cursorline_modes(&config, true, true, true);
        assert_eq!(
            config.editor.scroll_lines,
            helix_view::editor::Config::default().scroll_lines
        );
        assert!(warnings.iter().any(|warning| warning.path == "unknown"));
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "editor.mouse"));
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "editor.scroll-lines"));
    }

    #[test]
    fn invalid_toml_syntax_is_non_recoverable() {
        let err =
            Config::load_with_warnings(Ok("[editor".to_owned()), Err(ConfigLoadError::default()))
                .unwrap_err();

        assert!(matches!(err, ConfigLoadError::BadConfig(_)));
    }

    #[test]
    fn cursorline_bool_true_applies_to_all_modes() {
        let config = Config::load_test("editor.cursorline = true");

        assert_cursorline_modes(&config, true, true, true);
    }

    #[test]
    fn cursorline_bool_false_applies_to_all_modes() {
        let config = Config::load_test("editor.cursorline = false");

        assert_cursorline_modes(&config, false, false, false);
    }

    #[test]
    fn cursorline_per_mode_defaults_unspecified_modes_to_false() {
        let config = Config::load_test(
            r#"
            [editor.cursorline]
            normal = true
        "#,
        );

        assert_cursorline_modes(&config, true, false, false);
    }

    #[test]
    fn cursorline_per_mode_preserves_each_mode_value() {
        let config = Config::load_test(
            r#"
            [editor.cursorline]
            normal = true
            insert = false
            select = true
        "#,
        );

        assert_cursorline_modes(&config, true, false, true);
    }

    #[test]
    fn cursorline_ignores_unknown_nested_fields_with_non_bool_values() {
        let config = Config::load_test(
            r#"
            [editor.cursorline]
            normal = true
            completion-display = "statusline"
        "#,
        );

        assert_cursorline_modes(&config, true, false, false);
    }

    #[test]
    fn statusline_preserves_completion_suggestions_and_ignores_unknown_elements() {
        use helix_view::editor::StatusLineElement;

        let config = Config::load_test(
            r#"
            [editor.statusline]
            left = ["mode", "completion-suggestions", "mystery-element", "file-name"]
        "#,
        );

        assert_eq!(
            config.editor.statusline.left,
            vec![
                StatusLineElement::Mode,
                StatusLineElement::CompletionSuggestions,
                StatusLineElement::FileName
            ]
        );
    }
}
