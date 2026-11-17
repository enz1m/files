// Raw-publication fault injection preserves the production text framing.

#[cfg(test)]
use std::path::Path;

#[cfg(test)]
type TestRawWriter =
    Box<dyn FnOnce(&mut dyn std::io::Write, &[u8]) -> std::io::Result<()> + 'static>;

#[cfg(test)]
thread_local! {
    static TEST_RAW_WRITER: std::cell::RefCell<Option<TestRawWriter>> = const {
        std::cell::RefCell::new(None)
    };
}

// Route the next publication on this thread through `write`, which may stage a
// partial prefix and then fail.
#[cfg(test)]
pub(crate) fn inject_raw_writer(
    write: impl FnOnce(&mut dyn std::io::Write, &[u8]) -> std::io::Result<()> + 'static,
) {
    TEST_RAW_WRITER.with(|slot| {
        assert!(slot.borrow().is_none(), "a raw writer is already injected");
        *slot.borrow_mut() = Some(Box::new(write));
    });
}

// Publish `bytes` as the complete contents of `path`. Any failure before the
// rename leaves the previous file in place.

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) fn publish_raw(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(write) = TEST_RAW_WRITER.with(|slot| slot.borrow_mut().take()) {
        return enzui_core::atomic_state::save_atomic_raw_with(path, |staged| write(staged, bytes));
    }

    enzui_core::atomic_state::save_atomic_raw(path, bytes)
}
