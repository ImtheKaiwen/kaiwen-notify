//! Language resolution and the few strings only the native side shows (tray menu, tooltip).
//! All in-window text lives in the frontend dictionaries (`src/locales`).

#[cfg(test)]
pub const SUPPORTED: [&str; 2] = ["en", "tr"];

/// Primary language of the Windows display language, mapped to a supported code (default `en`).
pub fn system_language() -> &'static str {
    from_lang_id(windows_ui_language_id())
}

/// Maps a Windows LANGID to a supported language. Turkish is primary language 0x1F.
pub fn from_lang_id(lang_id: u16) -> &'static str {
    match lang_id & 0x03FF {
        0x1F => "tr",
        _ => "en",
    }
}

/// Resolves the stored preference (`system`, `en`, `tr`, anything else) to a supported code.
pub fn resolve(preference: &str) -> &'static str {
    match preference {
        "en" => "en",
        "tr" => "tr",
        _ => system_language(),
    }
}

#[cfg(windows)]
fn windows_ui_language_id() -> u16 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    unsafe { GetUserDefaultUILanguage() }
}

#[cfg(not(windows))]
fn windows_ui_language_id() -> u16 {
    0x09
}

pub struct TrayText {
    pub open: &'static str,
    pub test: &'static str,
    pub quit: &'static str,
    pub version_prefix: &'static str,
    pub update_available: &'static str,
}

pub fn tray_text(lang: &str) -> TrayText {
    match lang {
        "tr" => TrayText {
            open: "Ayarları aç",
            test: "Sesi test et",
            quit: "Tamamen çık",
            version_prefix: "Sürüm",
            update_available: "Güncelleme var",
        },
        _ => TrayText {
            open: "Open settings",
            test: "Test sound",
            quit: "Quit completely",
            version_prefix: "Version",
            update_available: "Update available",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turkish_and_english_ids() {
        assert_eq!(from_lang_id(0x041F), "tr"); // tr-TR
        assert_eq!(from_lang_id(0x0409), "en"); // en-US
        assert_eq!(from_lang_id(0x0407), "en"); // de-DE: unsupported falls back to English
        assert_eq!(from_lang_id(0x0000), "en");
    }

    #[test]
    fn explicit_preference_wins() {
        assert_eq!(resolve("tr"), "tr");
        assert_eq!(resolve("en"), "en");
    }

    #[test]
    fn tray_text_has_no_empty_entries() {
        for lang in SUPPORTED {
            let t = tray_text(lang);
            for s in [t.open, t.test, t.quit, t.version_prefix, t.update_available] {
                assert!(!s.is_empty());
            }
        }
    }
}
