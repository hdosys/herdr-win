//! Compile-time distribution configuration for herdr-win.

pub(crate) const CLI_VERSION_NAME: &str = "herdr-win";
pub(crate) const UPDATE_CHANNEL: &str = "preview";
pub(crate) const PREVIEW_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/hdosys/herdr-win/master/website/preview.json";
// Keep dormant upstream stable-feed code fork-owned so an accidental routing
// regression cannot contact upstream.
pub(crate) const STABLE_MANIFEST_URL: &str = PREVIEW_MANIFEST_URL;

#[cfg(windows)]
pub(crate) const WINDOWS_RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/hdosys/herdr-win/releases/download/";
