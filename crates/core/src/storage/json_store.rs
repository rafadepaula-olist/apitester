use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::models::{Collection, Environment};

/// Local-first, Git-friendly JSON file store for Collections and Environments.
#[derive(Debug, Clone)]
pub struct JsonStore {
    root_dir: PathBuf,
}

impl JsonStore {
    /// Creates a `JsonStore` rooted at `root_dir`, creating `collections/` and `environments/` subdirectories.
    pub fn open(root_dir: impl AsRef<Path>) -> CoreResult<Self> {
        let root_dir = root_dir.as_ref().to_path_buf();
        fs::create_dir_all(root_dir.join("collections"))?;
        fs::create_dir_all(root_dir.join("environments"))?;
        Ok(Self { root_dir })
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn collections_dir(&self) -> PathBuf {
        self.root_dir.join("collections")
    }

    pub fn environments_dir(&self) -> PathBuf {
        self.root_dir.join("environments")
    }

    pub fn save_collection(&self, collection: &Collection) -> CoreResult<PathBuf> {
        fs::create_dir_all(self.collections_dir())?;
        let path = self.collection_path(collection.id);
        write_json_atomic(&path, collection)?;
        Ok(path)
    }

    pub fn load_collection(&self, id: Uuid) -> CoreResult<Collection> {
        let path = self.collection_path(id);
        if !path.exists() {
            return Err(CoreError::NotFound(format!("collection {id}")));
        }
        let raw = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn list_collections(&self) -> CoreResult<Vec<Collection>> {
        let mut collections: Vec<Collection> = Vec::new();
        for entry in fs::read_dir(self.collections_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(col) = serde_json::from_str::<Collection>(&raw) {
                        collections.push(col);
                    }
                }
            }
        }
        collections.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(collections)
    }

    pub fn delete_collection(&self, id: Uuid) -> CoreResult<()> {
        let path = self.collection_path(id);
        if !path.exists() {
            return Err(CoreError::NotFound(format!("collection {id}")));
        }
        fs::remove_file(path)?;
        Ok(())
    }

    pub fn save_environment(&self, env: &Environment) -> CoreResult<PathBuf> {
        fs::create_dir_all(self.environments_dir())?;
        let path = self.environment_path(env.id);
        write_json_atomic(&path, env)?;
        Ok(path)
    }

    pub fn load_environment(&self, id: Uuid) -> CoreResult<Environment> {
        let path = self.environment_path(id);
        if !path.exists() {
            return Err(CoreError::NotFound(format!("environment {id}")));
        }
        let raw = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn list_environments(&self) -> CoreResult<Vec<Environment>> {
        let mut envs: Vec<Environment> = Vec::new();
        for entry in fs::read_dir(self.environments_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(env) = serde_json::from_str::<Environment>(&raw) {
                        envs.push(env);
                    }
                }
            }
        }
        envs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(envs)
    }

    pub fn delete_environment(&self, id: Uuid) -> CoreResult<()> {
        let path = self.environment_path(id);
        if !path.exists() {
            return Err(CoreError::NotFound(format!("environment {id}")));
        }
        fs::remove_file(path)?;
        Ok(())
    }

    fn collection_path(&self, id: Uuid) -> PathBuf {
        self.collections_dir().join(format!("{id}.json"))
    }

    fn environment_path(&self, id: Uuid) -> PathBuf {
        self.environments_dir().join(format!("{id}.json"))
    }
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> CoreResult<()> {
    let content = serde_json::to_string_pretty(value)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::models::{CollectionNode, HttpMethod, RequestItem, Variable};

    #[test]
    fn collection_crud_round_trip() {
        let dir = tempdir().expect("tempdir");
        let store = JsonStore::open(dir.path()).expect("open json store");

        let mut col = Collection::new("Payments API");
        col.items.push(CollectionNode::Request(RequestItem::new(
            "List Charges",
            HttpMethod::Get,
            "https://api.example.com/charges",
        )));

        store.save_collection(&col).expect("save");
        let loaded = store.load_collection(col.id).expect("load");
        assert_eq!(loaded, col);

        let all = store.list_collections().expect("list");
        assert_eq!(all.len(), 1);

        store.delete_collection(col.id).expect("delete");
        assert!(matches!(
            store.load_collection(col.id),
            Err(CoreError::NotFound(_))
        ));
    }

    #[test]
    fn environment_crud_round_trip() {
        let dir = tempdir().expect("tempdir");
        let store = JsonStore::open(dir.path()).expect("open json store");

        let mut env = Environment::new("Staging");
        env.variables
            .push(Variable::new("baseUrl", "https://staging.example.com"));

        store.save_environment(&env).expect("save");
        let loaded = store.load_environment(env.id).expect("load");
        assert_eq!(loaded, env);

        let all = store.list_environments().expect("list");
        assert_eq!(all.len(), 1);

        store.delete_environment(env.id).expect("delete");
        assert!(store.list_environments().expect("list").is_empty());
    }
}
