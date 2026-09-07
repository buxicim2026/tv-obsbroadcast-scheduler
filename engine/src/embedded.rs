//! Compile-time embedded assets: admin/ and overlay/ directories are baked
//! into the engine binary so the final distributable is a single file.

use include_dir::{include_dir, Dir};

/// `admin/` (broadcast scheduler admin UI) — served by axum at `/admin`.
pub static ADMIN_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../admin");

/// `overlay/` (broadcast status overlay) — served by axum at `/overlay`.
pub static OVERLAY_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../overlay");

/// Aggregate asset map so `server::build_router` doesn't have to enumerate.
/// Empty at this stage (init-scaffold); admin/overlay are filled by the
/// admin-web todo. Until then `/admin` and `/overlay` return 404 with a
/// friendly message.
#[derive(Debug, Clone, Copy, Default)]
pub struct DistAssets;

impl DistAssets {
    pub fn admin(&self) -> Option<&'static Dir<'static>> {
        // `files()` returns an iterator (no `is_empty`), and the iterator
        // borrows the Dir, so resolve it to a bool before returning `&Dir`.
        let empty = ADMIN_DIR.files().next().is_none();
        if empty {
            None
        } else {
            Some(&ADMIN_DIR)
        }
    }
    pub fn overlay(&self) -> Option<&'static Dir<'static>> {
        let empty = OVERLAY_DIR.files().next().is_none();
        if empty {
            None
        } else {
            Some(&OVERLAY_DIR)
        }
    }
}

pub fn dist() -> DistAssets {
    DistAssets::default()
}
