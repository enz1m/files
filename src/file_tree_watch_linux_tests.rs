// Linux integration test from an external filesystem mutation through the
// inotify subscription, reducer and completed directory-listing task.
//
// Missing inotify support is a failure; unsupported fallback has a separate
// deterministic test.

#![cfg(target_os = "linux")]
