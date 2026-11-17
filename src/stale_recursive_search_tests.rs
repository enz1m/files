// Stale recursive-search replies cannot replace the current folder.

use crate::message::Msg;
use crate::model::FileEntry;
use enzui_core::virtual_sequence::ItemProvider;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "files-1832-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn dir(&self, name: &str, files: &[&str]) -> PathBuf {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), file.as_bytes()).unwrap();
        }
        dir
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Vec<FileEntry> {
    crate::services::read_directory(path).unwrap()
}

fn visible_names(app: &crate::App) -> BTreeSet<String> {
    (0..app.model.visible_provider.len())
        .filter_map(|index| app.model.visible_provider.item(index))
        .filter_map(|row| app.model.projected_entry(&row))
        .map(|entry| entry.name.clone())
        .collect()
}

fn load(app: &mut crate::App, path: &Path) {
    let _ = app.update(Msg::NavigateTo(path.to_path_buf()));
    let generation = app.model.load_generation;
    let _ = app.update(Msg::DirLoaded {
        path: path.to_path_buf(),
        generation,
        result: Ok(read(path)),
    });
}

fn enter_recursive_search(app: &mut crate::App, query: &str) -> u64 {
    let _ = app.update(Msg::ToggleSearch);
    let _ = app.update(Msg::SearchChanged(query.to_owned()));
    let _ = app.update(Msg::SetSearchScope(true));
    app.model.search_generation
}

#[test]
fn same_root_same_query_supersession_drops_stale_walk() {
    let sandbox = Sandbox::new("same-query");
    let folder = sandbox.dir("A", &["match-a.txt", "match-b.txt", "other.txt"]);
    let matches: Vec<_> = read(&folder)
        .into_iter()
        .filter(|entry| entry.name.starts_with("match"))
        .collect();
    let (mut app, _) = crate::App::new();
    load(&mut app, &folder);

    let first = enter_recursive_search(&mut app, "match");
    let _ = app.update(Msg::SearchChanged("other".into()));
    let _ = app.update(Msg::SearchChanged("match".into()));
    let current = app.model.search_generation;
    assert!(current > first);

    let _ = app.update(Msg::SearchResultsReady {
        generation: current,
        root: folder.clone(),
        query: "match".into(),
        hits: matches,
    });
    let expected = BTreeSet::from(["match-a.txt".to_owned(), "match-b.txt".to_owned()]);
    assert_eq!(visible_names(&app), expected);

    // Same root and query, but an older generation: identity-only guards fail
    // this case and let the empty result erase the current projection.
    let _ = app.update(Msg::SearchResultsReady {
        generation: first,
        root: folder,
        query: "match".into(),
        hits: Vec::new(),
    });
    assert_eq!(visible_names(&app), expected);
}
