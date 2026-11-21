use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, WindowAppearance};
use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn override_appearance(self) -> Option<WindowAppearance> {
        match self {
            Self::System => None,
            Self::Light => Some(WindowAppearance::Light),
            Self::Dark => Some(WindowAppearance::Dark),
        }
    }

    fn mode(self, system: WindowAppearance) -> ThemeMode {
        self.override_appearance().unwrap_or(system).into()
    }

    pub fn apply(self, system: WindowAppearance, cx: &mut App) {
        let mode = self.mode(system);
        if Theme::global(cx).mode != mode {
            Theme::change(mode, None, cx);
        }
    }
}

pub fn appearance_path() -> Option<PathBuf> {
    dirs::config_dir().map(|root| root.join("Demolition").join("appearance"))
}

pub fn load_appearance(path: &Path) -> io::Result<Appearance> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(match text.trim() {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Appearance::System),
        Err(error) => Err(error),
    }
}

pub fn save_appearance(path: &Path, appearance: Appearance) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("No settings directory"))?;
    fs::create_dir_all(directory)?;
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    writeln!(temp, "{}", appearance.key())?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_system_and_saves_all_choices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Demolition/appearance");
        assert_eq!(load_appearance(&path).unwrap(), Appearance::System);
        for choice in [Appearance::Dark, Appearance::Light, Appearance::System] {
            save_appearance(&path, choice).unwrap();
            assert_eq!(load_appearance(&path).unwrap(), choice);
        }
        fs::write(&path, "unknown-theme").unwrap();
        assert_eq!(load_appearance(&path).unwrap(), Appearance::System);
    }

    #[test]
    fn only_system_mode_tracks_platform_appearance() {
        for (platform, expected) in [
            (WindowAppearance::Light, ThemeMode::Light),
            (WindowAppearance::VibrantLight, ThemeMode::Light),
            (WindowAppearance::Dark, ThemeMode::Dark),
            (WindowAppearance::VibrantDark, ThemeMode::Dark),
        ] {
            assert_eq!(Appearance::System.mode(platform), expected);
            assert_eq!(Appearance::Light.mode(platform), ThemeMode::Light);
            assert_eq!(Appearance::Dark.mode(platform), ThemeMode::Dark);
        }
    }
}
