//! What the user chose while the player ran (picture mode, lyrics view,
//! volume...), kept in the `settings` table so that it is there again the
//! next time. It wins over `config.toml`, which only gives the first values.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::db::Db;

const PREFIX: &str = "pref:";

#[derive(Default)]
pub struct Prefs {
    /// Values as loaded or last written.
    saved: HashMap<String, String>,
    db_path: Option<PathBuf>,
}

impl Prefs {
    pub fn load(db_path: PathBuf) -> (Self, Option<String>) {
        let rows = Db::open(&db_path).and_then(|db| db.settings_with_prefix(PREFIX));
        match rows {
            Ok(rows) => {
                let saved = rows
                    .into_iter()
                    .filter_map(|(k, v)| Some((k.strip_prefix(PREFIX)?.to_owned(), v)))
                    .collect();
                (
                    Self {
                        saved,
                        db_path: Some(db_path),
                    },
                    None,
                )
            }
            Err(e) => {
                tracing::error!("prefs: {e}");
                (
                    Self {
                        saved: HashMap::new(),
                        db_path: Some(db_path),
                    },
                    Some(format!("settings: {e}")),
                )
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.saved.get(key).map(String::as_str)
    }

    /// Write what differs from what is stored; returns an error text if the
    /// database cannot be written.
    pub fn store(&mut self, now: &[(&'static str, String)]) -> Option<String> {
        let changed: Vec<&(&str, String)> = now
            .iter()
            .filter(|(k, v)| self.saved.get(*k) != Some(v))
            .collect();
        if changed.is_empty() {
            return None;
        }
        let mut problem = None;
        if let Some(path) = &self.db_path {
            let written = Db::open(path).and_then(|db| {
                for (k, v) in &changed {
                    db.set_setting(&format!("{PREFIX}{k}"), v)?;
                }
                Ok(())
            });
            if let Err(e) = written {
                tracing::error!("cannot save settings: {e}");
                problem = Some(format!("cannot save settings: {e}"));
            }
        }
        for (k, v) in changed {
            self.saved.insert((*k).to_owned(), v.clone());
        }
        problem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_values_are_written_and_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.db");
        drop(Db::open(&db).unwrap());
        let (mut p, problem) = Prefs::load(db.clone());
        assert!(problem.is_none() && p.get("mode").is_none());
        assert!(
            p.store(&[("mode", "ascii".into()), ("fit", "fit".into())])
                .is_none()
        );
        // The same again writes nothing; a change writes just that.
        assert!(p.store(&[("mode", "ascii".into())]).is_none());
        let (again, _) = Prefs::load(db.clone());
        assert_eq!(again.get("mode"), Some("ascii"));
        assert_eq!(again.get("fit"), Some("fit"));
        let mut p = again;
        p.store(&[("mode", "blocks".into())]);
        let (last, _) = Prefs::load(db);
        assert_eq!(last.get("mode"), Some("blocks"));
        assert_eq!(last.get("fit"), Some("fit"));
    }

    #[test]
    fn a_broken_database_is_reported_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("bad.db");
        std::fs::write(&db, b"this is not a database at all").unwrap();
        let (mut p, problem) = Prefs::load(db);
        assert!(problem.is_some());
        assert!(p.get("x").is_none());
        // Writing fails quietly into the returned message.
        let _ = p.store(&[("x", "1".into())]);
    }
}
