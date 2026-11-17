use super::*;
use enzui_core::atomic_state::StateRefusal;
use enzui_core::persist::{InMemoryBackend, LoadOutcome, PersistBackend, PersistErrorKind};
use enzui_test_support::test_scratch::ScratchDir;
use std::fs;

fn sample() -> ViewSettings {
    let mut sort_overrides = BTreeMap::new();
    sort_overrides.insert(PathBuf::from("/home/u/proj"), (SortKey::Size, false));
    sort_overrides.insert(PathBuf::from("/tmp/with\ttab"), (SortKey::Modified, true));
    ViewSettings {
        view_mode: ViewMode::Grid,
        default_sort_key: SortKey::Kind,
        default_sort_ascending: false,
        sort_folders_first: false,
        show_hidden: true,
        colored_file_icons: false,
        list_zoom: 3,
        grid_zoom: 0,
        terminal_command: "foot --app-id=term".into(),
        folder_colors: crate::folder_colors::FolderColors::from_records(vec![
            crate::folder_colors::FolderColor {
                path: PathBuf::from("/home/u/with\ttab"),
                identity: crate::folder_colors::FolderIdentity {
                    dev: 2049,
                    ino: 77,
                    birth: Some((1_700_000_000, 5)),
                },
                color: enzui_theme::WorkspaceAccentKey::Rose,
            },
            crate::folder_colors::FolderColor {
                path: PathBuf::from("/home/u/old"),
                identity: crate::folder_colors::FolderIdentity {
                    dev: 2049,
                    ino: 78,
                    birth: None,
                },
                color: enzui_theme::WorkspaceAccentKey::Teal,
            },
        ]),
        tabs: vec![
            PathBuf::from("/home/u/proj"),
            PathBuf::from("/tmp/with\ttab"),
        ],
        active_tab: 1,
        single_click_open: true,
        visible_columns: [false, true, false],
        grid_info: [true, false, true],
        sort_overrides,
    }
}

#[test]
fn populated_preferences_survive_a_store_restart() {
    let scratch = ScratchDir::new("view-settings-restart");
    let path = scratch.path().join("view.state");
    let value = sample();
    let store = super::store(Some(&path));
    store.save_blocking(&value).unwrap();
    drop(store);
    assert_eq!(super::store(Some(&path)).load(), LoadOutcome::Loaded(value));
}

#[test]
fn future_version_is_a_write_blocking_header_refusal() {
    let backend = InMemoryBackend::new();
    backend
        .save_raw(
            FORMAT_ID,
            VERSION + 1,
            &sample().persist_serialize().unwrap(),
        )
        .unwrap();
    let store = store_over(backend);

    let outcome = store.load();

    assert!(matches!(
        outcome,
        LoadOutcome::Refused {
            fallback,
            reason: StateRefusal::UnexpectedHeader,
            ..
        } if fallback == ViewSettings::default()
    ));
    assert!(store.is_write_blocked());
}

fn store_over(backend: InMemoryBackend) -> Store<ViewSettings> {
    Store::new(backend, FORMAT_ID, VERSION)
}

fn framed_len_of(path: &Path, body: &str) -> u64 {
    let probe = FileBackend::new(path, RawByteLimit::new(u64::MAX));
    probe.save_raw(FORMAT_ID, VERSION, body).unwrap();
    fs::metadata(path).unwrap().len()
}

#[test]
fn real_store_save_at_raw_byte_limit_plus_one_refuses_and_preserves_last_good_bytes() {
    let scratch = ScratchDir::new("view-settings-plus-one");
    let probe_path = scratch.path().join("probe.state");
    let body = ViewSettings::default().persist_serialize().unwrap();
    let framed_len = framed_len_of(&probe_path, &body);

    let store_path = scratch.path().join("store.state");
    // A last-good frame gives the refused save something to preserve.
    FileBackend::new(&store_path, RawByteLimit::new(u64::MAX))
        .save_raw(FORMAT_ID, VERSION, &body)
        .unwrap();
    let last_good = fs::read(&store_path).unwrap();

    let over_limit = framed_len - 1;
    let store: Store<ViewSettings> = Store::new(
        FileBackend::new(&store_path, RawByteLimit::new(over_limit)),
        FORMAT_ID,
        VERSION,
    );
    let err = store
        .save_blocking(&ViewSettings::default())
        .expect_err("over-cap frame must refuse");
    assert_eq!(err.kind(), PersistErrorKind::Serialization);
    assert!(
        err.diagnostic().contains("too large"),
        "diagnostic must name the frame overflow: {:?}",
        err.diagnostic()
    );
    assert_eq!(
        fs::read(&store_path).unwrap(),
        last_good,
        "refused save must not alter last-good on-disk bytes"
    );
}
