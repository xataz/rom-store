use crate::i18n::Language;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    Grid,
    List,
}

impl ViewMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Grid => Self::List,
            Self::List => Self::Grid,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Frontend {
    #[default]
    None,
    Batocera,
    #[serde(alias = "es-de")]
    Esde,
    Recalbox,
    Emulationstation,
    Custom,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server_url: String,
    pub roms_dir: PathBuf,
    pub media_dir: Option<PathBuf>,
    pub gamelists_dir: Option<PathBuf>,
    pub system_mapping: BTreeMap<String, String>,
    pub cache_dir: PathBuf,
    pub font_path: PathBuf,
    pub fullscreen: bool,
    pub language: Language,
    pub games_view: ViewMode,
    pub systems_view: ViewMode,
    pub frontend: Frontend,
    pub auto_refresh: bool,
    /// Local executable and arguments; never interpreted by a shell.
    pub refresh_command: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:8000".into(),
            roms_dir: "/userdata/roms".into(),
            media_dir: None,
            gamelists_dir: None,
            system_mapping: BTreeMap::new(),
            cache_dir: std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
                        .join(".cache")
                })
                .join("romstore"),
            font_path: "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
            fullscreen: false,
            language: Language::default(),
            games_view: ViewMode::Grid,
            systems_view: ViewMode::List,
            frontend: Frontend::None,
            auto_refresh: false,
            refresh_command: Vec::new(),
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
                    .join(".config")
            })
            .join("romstore/config.yaml")
    }

    /// Validate before touching the destination, then atomically replace it.
    /// Existing symlinks keep pointing to the same configuration file.
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        self.validate()?;
        let contents = serde_saphyr::to_string(self)?;
        let target = if path.exists() {
            std::fs::canonicalize(path)?
        } else {
            path.to_path_buf()
        };
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("Le fichier de configuration est un lien symbolique sans cible valide");
        }
        let permissions = std::fs::metadata(&target).ok().map(|m| m.permissions());
        if permissions.as_ref().is_some_and(|p| p.readonly()) {
            bail!(
                "Le fichier de configuration est en lecture seule : {}",
                path.display()
            );
        }
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        if let Some(permissions) = permissions {
            temporary.as_file().set_permissions(permissions)?;
        }
        temporary.write_all(contents.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&target)
            .with_context(|| format!("Sauvegarde de {} impossible", path.display()))?;
        Ok(())
    }

    pub fn local_system<'a>(&'a self, server_system: &'a str) -> &'a str {
        self.system_mapping
            .get(server_system)
            .map(String::as_str)
            .unwrap_or(server_system)
    }

    pub fn rom_exists(&self, system: &str, path: &str) -> bool {
        crate::install::rom_exists(&self.roms_dir, self.local_system(system), path)
    }

    pub(crate) fn validate_layout(&self) -> Result<()> {
        for (server, local) in &self.system_mapping {
            crate::install::system_name(server)?;
            crate::install::system_name(local)?;
        }
        for path in [
            Some(&self.roms_dir),
            self.media_dir.as_ref(),
            self.gamelists_dir.as_ref(),
            Some(&self.cache_dir),
        ]
        .into_iter()
        .flatten()
        {
            if path.as_os_str().is_empty() {
                bail!("Les répertoires de bibliothèque ne doivent pas être vides");
            }
        }
        if self.frontend == Frontend::Esde
            && (self.media_dir.is_none() || self.gamelists_dir.is_none())
        {
            bail!("frontend: esde nécessite media_dir (downloaded_media) et gamelists_dir");
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        {
            bail!("La configuration utilise désormais YAML / Configuration now uses YAML: {} → config.yaml (server_url: ..., language: fr|en|es|de)", path.display());
        }
        let config: Self = serde_saphyr::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("Configuration introuvable : {}", path.display()))?,
        ).with_context(|| format!("Configuration YAML invalide / Invalid YAML configuration: {} (language: fr, en, es, de)", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.server_url)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("server_url doit être une URL HTTP(S) sans identifiants, paramètres ou fragment");
        }
        if self
            .refresh_command
            .first()
            .is_some_and(|program| program.trim().is_empty())
            || self.refresh_command.iter().any(|arg| arg.contains('\0'))
        {
            bail!("refresh_command doit contenir un exécutable non vide et des arguments sans caractère NUL");
        }
        if self.auto_refresh
            && self.frontend != Frontend::Batocera
            && self.refresh_command.is_empty()
        {
            bail!("auto_refresh nécessite frontend: batocera ou une refresh_command adaptée au frontend (ES-DE/Recalbox/autre)");
        }
        self.validate_layout()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_is_saved_and_loaded_with_its_type_and_exact_string_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config.yaml");
        let config = Config {
            server_url: "https://example.org:8443/prefix".into(),
            roms_dir: "./Mes ROMs".into(),
            media_dir: Some("/media/ES-DE".into()),
            gamelists_dir: Some("/home/user/gamelists".into()),
            system_mapping: BTreeMap::from([("gamecube".into(), "gc".into())]),
            cache_dir: "./cache".into(),
            font_path: "/Fonts/Ma Police.ttf".into(),
            fullscreen: true,
            language: Language::De,
            systems_view: ViewMode::Grid,
            games_view: ViewMode::List,
            frontend: Frontend::Esde,
            auto_refresh: true,
            refresh_command: vec![
                "/mon script".into(),
                "argument with spaces".into(),
                "true".into(),
                "".into(),
                "# : [quoted]".into(),
            ],
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn invalid_config_never_replaces_the_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        let original = b"# Configuration personnelle\nserver_url: http://localhost:8000\n";
        std::fs::write(&path, original).unwrap();
        let config = Config {
            server_url: "file:///etc/passwd".into(),
            ..Config::default()
        };
        assert!(config.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn read_only_config_is_not_silently_replaced_by_an_atomic_rename() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, "{}\n").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(Config::default()
            .save(&path)
            .unwrap_err()
            .to_string()
            .contains("lecture seule"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
    }

    #[cfg(unix)]
    #[test]
    fn saving_follows_existing_symlinks_without_replacing_them_and_rejects_dangling_links() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("actual.yaml");
        let link = directory.path().join("config.yaml");
        std::fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let config = Config::default();
        config.save(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(Config::load(&target).unwrap(), config);
        let dangling = directory.path().join("dangling.yaml");
        std::os::unix::fs::symlink(directory.path().join("absent.yaml"), &dangling).unwrap();
        assert!(config.save(&dangling).is_err());
        assert!(!directory.path().join("absent.yaml").exists());
    }

    fn load(text: &str) -> Result<Config> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.yaml");
        std::fs::write(&path, text)?;
        Config::load(&path)
    }

    #[test]
    fn yaml_reads_all_fields_and_preserves_relative_paths() {
        let config = load("server_url: http://localhost:6789\nroms_dir: ./my roms\ncache_dir: ./cache\nfont_path: /fonts/custom.ttf\nfullscreen: true\nlanguage: en\n").unwrap();
        assert_eq!(config.server_url, "http://localhost:6789");
        assert_eq!(config.roms_dir, PathBuf::from("./my roms"));
        assert_eq!(config.cache_dir, PathBuf::from("./cache"));
        assert_eq!(config.font_path, PathBuf::from("/fonts/custom.ttf"));
        assert!(config.fullscreen);
        assert_eq!(config.language, Language::En);
    }

    #[test]
    fn defaults_and_supported_languages() {
        assert_eq!(load("{}").unwrap().language, Language::Fr);
        assert_eq!(load("{}").unwrap().games_view, ViewMode::Grid);
        assert_eq!(load("{}").unwrap().systems_view, ViewMode::List);
        let config = load("games_view: list\nsystems_view: grid").unwrap();
        assert_eq!(config.games_view, ViewMode::List);
        assert_eq!(config.systems_view, ViewMode::Grid);
        for (code, expected) in [
            ("fr", Language::Fr),
            ("en", Language::En),
            ("es", Language::Es),
            ("de", Language::De),
        ] {
            assert_eq!(
                load(&format!("language: {code}")).unwrap().language,
                expected
            );
        }
        // Both conventional extensions work.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yml");
        std::fs::write(&path, "language: de").unwrap();
        assert_eq!(Config::load(&path).unwrap().language, Language::De);
    }

    #[test]
    fn invalid_yaml_unknown_fields_languages_and_unsafe_urls_are_rejected() {
        for text in [
            "language: [",
            "language: it",
            "unexpected: value",
            "fullscreen: maybe",
            "games_view: invalid",
            "systems_view: tiles",
            "server_url: file:///etc/passwd",
            "server_url: http://user:pass@example.com",
            "server_url: http://example.com?secret=1",
        ] {
            assert!(load(text).is_err(), "Should reject {text}");
        }
    }

    #[test]
    fn old_toml_configuration_has_a_migration_hint() {
        let error = Config::load(Path::new("config.toml"))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("config.yaml"));
        assert!(error.contains("YAML"));
    }

    #[test]
    fn frontend_refresh_is_explicit_and_existing_configs_keep_it_disabled() {
        let config = load("{}").unwrap();
        assert_eq!(config.frontend, Frontend::None);
        assert!(!config.auto_refresh);
        assert!(config.refresh_command.is_empty());
        let config = load("frontend: batocera\nauto_refresh: true").unwrap();
        assert_eq!(config.frontend, Frontend::Batocera);
        assert!(config.auto_refresh);
        for frontend in [
            "esde",
            "es-de",
            "recalbox",
            "emulationstation",
            "custom",
            "none",
        ] {
            let layout = "media_dir: ./downloaded_media\ngamelists_dir: ./gamelists\n";
            let config = load(&format!("{layout}frontend: {frontend}\nauto_refresh: true\nrefresh_command: ['/my refresh script', 'argument with spaces']")).unwrap();
            assert_eq!(
                config.refresh_command,
                ["/my refresh script", "argument with spaces"]
            );
            assert!(load(&format!("frontend: {frontend}\nauto_refresh: true")).is_err());
            assert!(load(&format!(
                "{layout}frontend: {frontend}\nauto_refresh: false"
            ))
            .is_ok());
        }
        for text in [
            "frontend: unknown",
            "auto_refresh: maybe",
            "refresh_command: 'shell command'",
            "refresh_command: ['']",
        ] {
            assert!(load(text).is_err(), "Must reject {text}");
        }
    }

    #[test]
    fn esde_paths_and_system_mapping_are_explicit_and_validated() {
        let config = load("frontend: es-de\nmedia_dir: ./ES-DE/downloaded_media\ngamelists_dir: ./ES-DE/gamelists\nsystem_mapping:\n  gamecube: gc\n  sg1000: sg-1000\n").unwrap();
        assert_eq!(config.local_system("gamecube"), "gc");
        assert_eq!(config.local_system("sg1000"), "sg-1000");
        assert_eq!(config.local_system("nes"), "nes");
        assert_eq!(
            config.media_dir.unwrap(),
            PathBuf::from("./ES-DE/downloaded_media")
        );
        assert!(!config.auto_refresh);
        for yaml in [
            "frontend: esde",
            "frontend: esde\nmedia_dir: ./media",
            "frontend: esde\ngamelists_dir: ./gamelists",
            "media_dir: ''",
            "gamelists_dir: ''",
            "system_mapping: {gamecube: ../gc}",
            "system_mapping: {../gamecube: gc}",
            "system_mapping: {gamecube: 'gc/subdir'}",
            "system_mapping: {gamecube: .romstore}",
            "system_mapping: {gamecube: gamelist.xml}",
        ] {
            assert!(load(yaml).is_err(), "Must reject {yaml}");
        }
    }
}
