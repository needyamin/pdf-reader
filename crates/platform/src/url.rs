//! Handing a URL to the default browser.
//!
//! Used by the lookup popup's Google Translate button. The URL is always built
//! by the lookup crate, so the only check here is that it really is a web
//! address — a `file:` or worse URL must never reach the shell.

use std::io::Error as IoError;
use std::process::Command;

/// Why a URL could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum UrlError {
    /// The URL was not an `http(s)` address.
    #[error("refusing to open a non-web URL")]
    NotWeb,
    /// The browser could not be started.
    #[error("could not open the browser: {0}")]
    Spawn(#[from] IoError),
}

/// Open `url` in the user's default browser.
pub fn open(url: &str) -> Result<(), UrlError> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(UrlError::NotWeb);
    }

    #[cfg(target_os = "windows")]
    {
        // Open through `rundll32` rather than `cmd /c start`. The translate
        // link is full of `&` characters, and `&` is a command separator in
        // `cmd` — `start` would therefore only see the URL up to the first
        // `&`, so Google Translate would open without the selected text.
        // `rundll32` takes the URL as a plain argv element, so the `&` signs
        // are never interpreted by a shell.
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_urls_are_allowed_through() {
        assert!(matches!(open("file:///C:/secret"), Err(UrlError::NotWeb)));
        assert!(matches!(open("not-a-url"), Err(UrlError::NotWeb)));
        assert!(matches!(open("javascript:alert(1)"), Err(UrlError::NotWeb)));
    }
}
