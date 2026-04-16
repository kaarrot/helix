use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::{document::Mode, theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use toml::de::Error as TomlError;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub editor: helix_view::editor::Config,
    /// Non-fatal warnings produced while loading (e.g. unknown fields skipped).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
            warnings: Vec::new(),
        }
    }
}

/// Try to deserialize editor config, stripping unknown fields one-by-one and
/// collecting a warning for each one rather than failing the whole load.
fn parse_editor_lenient(
    val: toml::Value,
) -> Result<(helix_view::editor::Config, Vec<String>), toml::de::Error> {
    let mut val = val;
    let mut warnings = Vec::new();
    loop {
        match val.clone().try_into::<helix_view::editor::Config>() {
            Ok(config) => return Ok((config, warnings)),
            Err(err) => {
                // toml errors for unknown fields look like:
                // "unknown field `foo`, expected one of ..."
                let msg = err.to_string();
                if let Some(field) = unknown_field_name(&msg) {
                    warnings.push(format!(
                        "Unknown editor config field ignored: `{field}`"
                    ));
                    if let toml::Value::Table(ref mut table) = val {
                        table.remove(field);
                    } else {
                        return Err(err);
                    }
                } else {
                    return Err(err);
                }
            }
        }
    }
}

fn unknown_field_name(err: &str) -> Option<&str> {
    let prefix = "unknown field `";
    let start = err.find(prefix)? + prefix.len();
    let end = err[start..].find('`')? + start;
    Some(&err[start..end])
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
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

impl Config {
    pub fn load(
        global: Result<String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<Config, ConfigLoadError> {
        let global_config: Result<ConfigRaw, ConfigLoadError> =
            global.and_then(|file| toml::from_str(&file).map_err(ConfigLoadError::BadConfig));
        let local_config: Result<ConfigRaw, ConfigLoadError> =
            local.and_then(|file| toml::from_str(&file).map_err(ConfigLoadError::BadConfig));
        let res = match (global_config, local_config) {
            (Ok(global), Ok(local)) => {
                let mut keys = keymap::default();
                if let Some(global_keys) = global.keys {
                    merge_keys(&mut keys, global_keys)
                }
                if let Some(local_keys) = local.keys {
                    merge_keys(&mut keys, local_keys)
                }

                let (editor, warnings) = match (global.editor, local.editor) {
                    (None, None) => (helix_view::editor::Config::default(), Vec::new()),
                    (None, Some(val)) | (Some(val), None) => {
                        parse_editor_lenient(val).map_err(ConfigLoadError::BadConfig)?
                    }
                    (Some(global), Some(local)) => {
                        parse_editor_lenient(merge_toml_values(global, local, 3))
                            .map_err(ConfigLoadError::BadConfig)?
                    }
                };

                Config {
                    theme: local.theme.or(global.theme),
                    keys,
                    editor,
                    warnings,
                }
            }
            // if any configs are invalid return that first
            (_, Err(ConfigLoadError::BadConfig(err)))
            | (Err(ConfigLoadError::BadConfig(err)), _) => {
                return Err(ConfigLoadError::BadConfig(err))
            }
            (Ok(config), Err(_)) | (Err(_), Ok(config)) => {
                let mut keys = keymap::default();
                if let Some(keymap) = config.keys {
                    merge_keys(&mut keys, keymap);
                }
                let (editor, warnings) = config
                    .editor
                    .map_or_else(
                        || Ok((helix_view::editor::Config::default(), Vec::new())),
                        |val| parse_editor_lenient(val).map_err(ConfigLoadError::BadConfig),
                    )?;
                Config {
                    theme: config.theme,
                    keys,
                    editor,
                    warnings,
                }
            }

            // these are just two io errors return the one for the global config
            (Err(err), Err(_)) => return Err(err),
        };

        Ok(res)
    }

    pub fn load_default() -> Result<Config, ConfigLoadError> {
        let global_config =
            fs::read_to_string(helix_loader::config_file()).map_err(ConfigLoadError::Error);
        let local_config = fs::read_to_string(helix_loader::workspace_config_file())
            .map_err(ConfigLoadError::Error);
        Config::load(global_config, local_config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Config {
        fn load_test(config: &str) -> Config {
            Config::load(Ok(config.to_owned()), Err(ConfigLoadError::default())).unwrap()
        }
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
    fn cursorline_ignores_unknown_nested_fields_with_non_bool_values() {
        use helix_view::document::Mode;

        let config = Config::load_test(
            r#"
            [editor.cursorline]
            normal = true
            completion-display = "statusline"
        "#,
        );

        assert!(config.editor.cursorline.from_mode(Mode::Normal));
        assert!(!config.editor.cursorline.from_mode(Mode::Insert));
        assert!(!config.editor.cursorline.from_mode(Mode::Select));
    }

    #[test]
    fn statusline_ignores_unknown_elements() {
        use helix_view::editor::StatusLineElement;

        let config = Config::load_test(
            r#"
            [editor.statusline]
            left = ["mode", "completion-suggestions", "file-name"]
        "#,
        );

        assert_eq!(
            config.editor.statusline.left,
            vec![StatusLineElement::Mode, StatusLineElement::FileName]
        );
    }
}
