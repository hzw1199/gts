use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct HomeOptions {
    pub home: Option<PathBuf>,
}

impl HomeOptions {
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
        }
    }
}

pub fn resolve_gts_home(opts: &HomeOptions) -> PathBuf {
    if let Some(home) = &opts.home {
        if !home.as_os_str().is_empty() {
            return PathBuf::from(path_absolutize(home));
        }
    }
    if let Ok(env_home) = env::var("GTS_HOME") {
        if !env_home.is_empty() {
            return PathBuf::from(path_absolutize(Path::new(&env_home)));
        }
    }
    let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    user_home.join(".gts")
}

fn path_absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

pub fn index_path(home: &Path) -> PathBuf {
    home.join("index.json")
}

pub fn storage_root(home: &Path) -> PathBuf {
    home.join("storage")
}

pub fn workspace_storage_dir(home: &Path, id: &str) -> PathBuf {
    storage_root(home).join(id)
}

pub fn track_git_dir(home: &Path, id: &str, track: &str) -> PathBuf {
    workspace_storage_dir(home, id).join(track).join(".git")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn uses_explicit_home_over_env_and_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = temp_dir("gts-home-opt");
        let prev = env::var("GTS_HOME").ok();
        unsafe { env::set_var("GTS_HOME", temp_dir("gts-should-not-use")) };
        let resolved = resolve_gts_home(&HomeOptions::with_home(&tmp));
        assert_eq!(resolved, path_absolutize(&tmp));
        match prev {
            Some(v) => unsafe { env::set_var("GTS_HOME", v) },
            None => unsafe { env::remove_var("GTS_HOME") },
        }
    }

    #[test]
    fn uses_gts_home_when_option_omitted() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = temp_dir("gts-home-env");
        let prev = env::var("GTS_HOME").ok();
        unsafe { env::set_var("GTS_HOME", &tmp) };
        let resolved = resolve_gts_home(&HomeOptions::default());
        assert_eq!(resolved, path_absolutize(&tmp));
        match prev {
            Some(v) => unsafe { env::set_var("GTS_HOME", v) },
            None => unsafe { env::remove_var("GTS_HOME") },
        }
    }

    #[test]
    fn defaults_to_dot_gts_without_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = env::var("GTS_HOME").ok();
        unsafe { env::remove_var("GTS_HOME") };
        let expected = dirs::home_dir().unwrap().join(".gts");
        assert_eq!(resolve_gts_home(&HomeOptions::default()), expected);
        if let Some(v) = prev {
            unsafe { env::set_var("GTS_HOME", v) };
        }
    }
}
