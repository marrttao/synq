//! Resolution of the BASE directory and the folder layout under it.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Environment variable that overrides the BASE directory.
pub const HOME_ENV: &str = "PLAYER_HOME";

/// All well-known locations of the player, derived from BASE.
#[derive(Debug, Clone)]
pub struct Paths {
    pub base: PathBuf,
    pub music: PathBuf,
    pub gifs: PathBuf,
    pub playlists: PathBuf,
    pub favorites: PathBuf,
    pub data: PathBuf,
    #[allow(dead_code)] // used from stage 2 (library database)
    pub db: PathBuf,
    pub lyrics: PathBuf,
    pub models: PathBuf,
    pub logs: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    pub fn new(base: PathBuf) -> Self {
        let data = base.join("data");
        Self {
            music: base.join("music"),
            gifs: base.join("gifs"),
            playlists: base.join("playlists"),
            favorites: base.join("favorites"),
            db: data.join("library.db"),
            lyrics: data.join("lyrics"),
            models: data.join("models"),
            logs: data.join("logs"),
            config: base.join("config.toml"),
            data,
            base,
        }
    }

    /// Resolve BASE from the real environment: PLAYER_HOME, the executable's
    /// folder (if it contains `music/`), or the current working directory.
    pub fn detect() -> io::Result<Self> {
        let home = env::var_os(HOME_ENV).map(PathBuf::from);
        let exe_dir = env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let cwd = env::current_dir()?;
        Ok(Self::new(resolve_base(home, exe_dir, &cwd)))
    }

    /// Create every missing folder. Existing content is never touched.
    pub fn ensure_dirs(&self) -> io::Result<()> {
        for dir in [
            &self.music,
            &self.gifs,
            &self.playlists,
            &self.favorites,
            &self.data,
            &self.lyrics,
            &self.models,
            &self.logs,
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Pure BASE resolution, separated from the environment for testing.
pub fn resolve_base(home: Option<PathBuf>, exe_dir: Option<PathBuf>, cwd: &Path) -> PathBuf {
    if let Some(home) = home.filter(|p| !p.as_os_str().is_empty()) {
        return home;
    }
    if let Some(dir) = exe_dir.filter(|d| d.join("music").is_dir()) {
        return dir;
    }
    cwd.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_wins() {
        let exe = tempfile::tempdir().unwrap();
        fs::create_dir(exe.path().join("music")).unwrap();
        let got = resolve_base(
            Some(PathBuf::from("/custom/home")),
            Some(exe.path().to_path_buf()),
            Path::new("/cwd"),
        );
        assert_eq!(got, PathBuf::from("/custom/home"));
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let got = resolve_base(Some(PathBuf::new()), None, Path::new("/cwd"));
        assert_eq!(got, PathBuf::from("/cwd"));
    }

    #[test]
    fn exe_dir_used_when_it_has_music() {
        let exe = tempfile::tempdir().unwrap();
        fs::create_dir(exe.path().join("music")).unwrap();
        let got = resolve_base(None, Some(exe.path().to_path_buf()), Path::new("/cwd"));
        assert_eq!(got, exe.path());
    }

    #[test]
    fn falls_back_to_cwd_without_music_next_to_exe() {
        let exe = tempfile::tempdir().unwrap();
        let got = resolve_base(None, Some(exe.path().to_path_buf()), Path::new("/cwd"));
        assert_eq!(got, PathBuf::from("/cwd"));
    }

    #[test]
    fn ensure_dirs_creates_layout_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path().to_path_buf());
        paths.ensure_dirs().unwrap();
        paths.ensure_dirs().unwrap();
        for dir in [
            &paths.music,
            &paths.gifs,
            &paths.playlists,
            &paths.favorites,
            &paths.lyrics,
            &paths.models,
            &paths.logs,
        ] {
            assert!(dir.is_dir(), "{} missing", dir.display());
        }
    }
}
