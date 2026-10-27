//! Écriture de `config.toml` par la page Paramètres (specs §11.2) : le fichier
//! reste la source de vérité, ses commentaires et sa mise en forme sont gardés.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

use crate::Profile;

pub struct ConfigFile {
    path: PathBuf,
    doc: DocumentMut,
}

impl ConfigFile {
    /// Ouvre le fichier ; absent, il sera créé à l'enregistrement.
    pub fn open(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("lecture de {}", path.display())),
        };
        let doc = text.parse().with_context(|| format!("lecture de {}", path.display()))?;
        Ok(Self { path: path.to_owned(), doc })
    }

    /// `keys` : `["terminal", "scrollback"]`. Les tables manquantes sont créées.
    pub fn set(&mut self, keys: &[&str], value: impl Into<Value>) {
        let Some((last, tables)) = keys.split_last() else { return };
        let mut table = self.doc.as_table_mut();
        for key in tables {
            let item = table.entry(key).or_insert_with(|| Item::Table(Table::new()));
            if !item.is_table() {
                *item = Item::Table(Table::new());
            }
            table = item.as_table_mut().expect("table créée");
        }
        match table.get_mut(last).and_then(Item::as_value_mut) {
            // Remplacer la valeur seulement : le commentaire de fin de ligne reste.
            Some(existing) => {
                let decor = existing.decor().clone();
                *existing = value.into();
                *existing.decor_mut() = decor;
            }
            None => {
                table.insert(last, Item::Value(value.into()));
            }
        }
    }

    /// Retire une clé : la valeur par défaut s'applique.
    pub fn remove(&mut self, keys: &[&str]) {
        let Some((last, tables)) = keys.split_last() else { return };
        let mut table = self.doc.as_table_mut();
        for key in tables {
            match table.get_mut(key).and_then(Item::as_table_mut) {
                Some(next) => table = next,
                None => return,
            }
        }
        table.remove(last);
    }

    fn profiles(&mut self) -> &mut ArrayOfTables {
        let terminal = self.doc.entry("terminal").or_insert_with(|| Item::Table(Table::new()));
        if !terminal.is_table() {
            *terminal = Item::Table(Table::new());
        }
        let terminal = terminal.as_table_mut().expect("table terminal");
        let profiles = terminal.entry("profiles").or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
        if !profiles.is_array_of_tables() {
            *profiles = Item::ArrayOfTables(ArrayOfTables::new());
        }
        profiles.as_array_of_tables_mut().expect("tableau de profils")
    }

    /// Enregistre `profile` à la place de celui nommé `previous` (renommage),
    /// sinon à la place de celui du même nom, sinon à la fin.
    pub fn set_profile(&mut self, previous: Option<&str>, profile: &Profile) -> Result<()> {
        let text = toml::to_string(profile).context("profil")?;
        let mut table = text.parse::<DocumentMut>().context("profil")?.as_table().clone();
        let names = [previous.unwrap_or(&profile.name), &profile.name];
        let profiles = self.profiles();
        let index = names.iter().find_map(|name| {
            profiles.iter().position(|t| t.get("name").and_then(Item::as_str) == Some(name))
        });
        match index.and_then(|i| profiles.get_mut(i)) {
            Some(existing) => {
                // Les commentaires au-dessus du profil restent.
                *table.decor_mut() = existing.decor().clone();
                *existing = table;
            }
            None => {
                table.set_implicit(false);
                profiles.push(table);
            }
        }
        Ok(())
    }

    pub fn remove_profile(&mut self, name: &str) {
        let profiles = self.profiles();
        let index = profiles.iter().position(|t| t.get("name").and_then(Item::as_str) == Some(name));
        if let Some(index) = index {
            profiles.remove(index);
        }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
        }
        // Fichier temporaire puis renommage : jamais de fichier à moitié écrit.
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.doc.to_string()).with_context(|| format!("écriture de {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("écriture de {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn keeps_comments_and_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# Ma config\n[font]\nfamily = \"Iosevka\" # ma police\nsize = 13.0\n\n# profils\n[[terminal.profiles]]\nname = \"db\"\ncommand = \"psql\"\n",
        )
        .unwrap();

        let mut file = ConfigFile::open(&path).unwrap();
        file.set(&["font", "size"], 15.0);
        file.set(&["terminal", "scrollback"], 5000);
        file.set(&["claude", "notifications"], false);
        file.set_profile(Some("db"), &Profile { name: "base".into(), command: Some("psql -U app".into()), ..Default::default() })
            .unwrap();
        file.set_profile(None, &Profile::ssh("web")).unwrap();
        file.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Ma config\n[font]\nfamily = \"Iosevka\" # ma police\nsize = 15.0\n"), "{text}");
        assert!(text.contains("# profils\n[[terminal.profiles]]\nname = \"base\""), "{text}");
        let config = Config::load(&path).unwrap();
        assert_eq!(config.font.size, 15.0);
        assert_eq!(config.terminal.scrollback, 5000);
        assert!(!config.claude.notifications);
        let names: Vec<_> = config.terminal.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["base", "web"]);

        let mut file = ConfigFile::open(&path).unwrap();
        file.remove_profile("base");
        file.remove(&["font", "size"]);
        file.save().unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.terminal.profiles, [Profile::ssh("web")]);
    }

    #[test]
    fn creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grove/config.toml");
        let mut file = ConfigFile::open(&path).unwrap();
        file.set(&["terminal", "default_profile"], "web");
        file.save().unwrap();
        assert_eq!(Config::load(&path).unwrap().terminal.default_profile.as_deref(), Some("web"));
    }
}
