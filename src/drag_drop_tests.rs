use super::*;

use std::fs;
use std::path::{Path, PathBuf};

use enzui_core::drag::{
    DragOperation, DropRequest, DropTarget, InsertionSlot, ItemDropRequest, ItemId,
};

use crate::drag_drop;

struct Sandbox(PathBuf);
impl Sandbox {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "files-app-dnd-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        Sandbox(base)
    }
    fn root(&self) -> &Path {
        &self.0
    }
    fn dir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn id_of(p: &Path) -> ItemId {
    // The validator checks membership against `path_to_id` ids, as the views
    // encode them.
    drag_drop::path_to_id(p)
}

fn pointer_drop(items: &[&Path], target: DropTarget, op: DragOperation) -> ItemDropRequest {
    ItemDropRequest {
        request: DropRequest {
            source: drag_drop::collection(),
            destination: drag_drop::collection(),
            items: items.iter().map(|p| id_of(p)).collect(),
            slot: InsertionSlot(0),
            operation: op,
        },
        target,
    }
}

fn app_on(dir: &Path) -> App {
    let (mut app, _t) = App::new();
    app.model.screen = Screen::Directory;
    app.model.cwd = dir.to_path_buf();
    app.model
        .set_entries(services::read_directory(dir).unwrap());
    app
}

#[test]
fn pointer_drop_moves_only_the_selected_native_path() {
    use std::os::unix::ffi::OsStringExt;
    let sb = Sandbox::new("routemove");
    let name = std::ffi::OsString::from_vec(b"payload-\xff.txt".to_vec());
    let other_name = std::ffi::OsString::from_vec(b"payload-\xfe.txt".to_vec());
    let file = sb.root().join(&name);
    let other = sb.root().join(&other_name);
    fs::write(&file, b"selected bytes").unwrap();
    fs::write(&other, b"unselected canary").unwrap();
    assert_eq!(name.to_string_lossy(), other_name.to_string_lossy());
    let folder = sb.dir("dropzone");
    let mut app = app_on(sb.root());
    let req = pointer_drop(
        &[&file],
        DropTarget::OntoItem(id_of(&folder)),
        DragOperation::Move,
    );
    crate::file_clipboard_tests::dispatch(&mut app, Msg::FileItemDropped(req));
    assert_eq!(fs::read(folder.join(name)).unwrap(), b"selected bytes");
    assert!(!file.exists(), "moved source must be retired");
    assert_eq!(fs::read(other).unwrap(), b"unselected canary");
    assert!(!folder.join(other_name).exists());
}
