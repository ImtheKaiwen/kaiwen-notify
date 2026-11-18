//! Per-provider sound settings, the private copy store for custom sounds, and the one-time migration from the
//! single shared sound of earlier versions.
//!
//! Custom sounds are copied into the app's own data folder under content-derived names, so moving or deleting
//! the original never breaks a notification, and the UI/IPC never carry a path that gets executed or traversed.

use crate::{audio, events::Provider};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

/// `sounds_version` written after the migration below ran.
pub const SOUNDS_VERSION: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoundKind {
    Main,
    Subagent,
}

impl SoundKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(Self::Main),
            "subagent" => Some(Self::Subagent),
            _ => None,
        }
    }
}

/// One selectable sound. An empty `file` means "the bundled default for this kind".
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct SoundChoice {
    /// File name inside the sounds folder (never a path). Empty = bundled default.
    pub file: String,
    /// Display name of the original file, shown in the UI.
    pub name: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct ProviderSound {
    pub enabled: bool,
    /// 0..=100
    pub volume: u8,
    pub main: SoundChoice,
    pub subagent: SoundChoice,
}

impl Default for ProviderSound {
    fn default() -> Self {
        Self {
            enabled: true,
            volume: 100,
            main: SoundChoice::default(),
            subagent: SoundChoice::default(),
        }
    }
}

impl ProviderSound {
    pub fn choice(&self, kind: SoundKind) -> &SoundChoice {
        match kind {
            SoundKind::Main => &self.main,
            SoundKind::Subagent => &self.subagent,
        }
    }

    pub fn choice_mut(&mut self, kind: SoundKind) -> &mut SoundChoice {
        match kind {
            SoundKind::Main => &mut self.main,
            SoundKind::Subagent => &mut self.subagent,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct ProviderSounds {
    pub claude: ProviderSound,
    pub codex: ProviderSound,
    pub antigravity: ProviderSound,
}

impl ProviderSounds {
    pub fn get(&self, provider: Provider) -> &ProviderSound {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
            Provider::Antigravity => &self.antigravity,
        }
    }

    pub fn get_mut(&mut self, provider: Provider) -> &mut ProviderSound {
        match provider {
            Provider::Claude => &mut self.claude,
            Provider::Codex => &mut self.codex,
            Provider::Antigravity => &mut self.antigravity,
        }
    }

    fn all_mut(&mut self) -> [&mut ProviderSound; 3] {
        [&mut self.claude, &mut self.codex, &mut self.antigravity]
    }

    /// Every stored file any choice still points at.
    pub fn referenced_files(&self) -> HashSet<String> {
        [&self.claude, &self.codex, &self.antigravity]
            .iter()
            .flat_map(|p| [p.main.file.clone(), p.subagent.file.clone()])
            .filter(|f| !f.is_empty())
            .collect()
    }
}

/// `<16 hex digits>.<wav|mp3|ogg>`: the only names this store ever creates or accepts. Anything else (separators,
/// drive letters, `..`) is refused, so a hand-edited settings file cannot point playback outside the folder.
pub fn is_store_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    stem.len() == 16
        && stem
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && audio::ALLOWED_EXTENSIONS.contains(&ext)
}

/// Resolves a choice to a file inside `dir`; `None` means "use the bundled default".
pub fn resolve(dir: &Path, choice: &SoundChoice) -> Option<PathBuf> {
    (!choice.file.is_empty() && is_store_name(&choice.file)).then(|| dir.join(&choice.file))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn display_name(source: &Path) -> String {
    let raw = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    raw.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// Validates a user-picked file (type, size, really decodable, length) and copies it into the store.
/// The write is atomic (temp file then rename). Identical content is stored once.
pub fn import(source: &Path, dir: &Path) -> Result<SoundChoice, String> {
    let extension = audio::allowed_extension(source).ok_or("E_SOUND_INVALID")?;
    let meta = fs::metadata(source).map_err(|_| "E_SOUND_MISSING".to_string())?;
    if !meta.is_file() {
        return Err("E_SOUND_MISSING".into());
    }
    if meta.len() > audio::MAX_FILE_BYTES {
        return Err("E_SOUND_TOO_LARGE".into());
    }
    let bytes = fs::read(source).map_err(|_| "E_SOUND_MISSING".to_string())?;
    audio::decode_bytes(bytes.clone(), &extension)?; // refuses anything that is not real, short audio
    fs::create_dir_all(dir).map_err(|e| format!("E_SAVE_SETTINGS:{e}"))?;
    let file = format!("{:016x}.{extension}", fnv1a(&bytes));
    let target = dir.join(&file);
    if !target.is_file() {
        let tmp = dir.join(format!("{file}.tmp"));
        fs::write(&tmp, &bytes)
            .and_then(|_| fs::rename(&tmp, &target))
            .map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("E_SAVE_SETTINGS:{e}")
            })?;
    }
    Ok(SoundChoice {
        file,
        name: display_name(source),
    })
}

/// Deletes stored sound files nothing references any more. Only files this store could have created are
/// considered; anything else in the folder is left alone.
pub fn cleanup(dir: &Path, sounds: &ProviderSounds) {
    let keep = sounds.referenced_files();
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_store_name(&name) && !keep.contains(&name) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// One-time migration: the single shared sound of earlier versions becomes the main sound of every provider,
/// so nobody loses their custom sound. The legacy field is left untouched in the file (downgrade-safe).
/// Returns true when the migration ran.
pub fn migrate(
    legacy_sound_path: &str,
    version: &mut u32,
    sounds: &mut ProviderSounds,
    dir: &Path,
) -> bool {
    if *version >= SOUNDS_VERSION {
        return false;
    }
    *version = SOUNDS_VERSION;
    if legacy_sound_path.is_empty() {
        return true;
    }
    // A moved or deleted legacy file simply stays "default": it was silent-failing before as well.
    if let Ok(choice) = import(Path::new(legacy_sound_path), dir) {
        for provider in sounds.all_mut() {
            provider.main = choice.clone();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAV: &[u8] = include_bytes!("../test-fixtures/tone.wav");
    const MP3: &[u8] = include_bytes!("../test-fixtures/tone.mp3");
    const OGG: &[u8] = include_bytes!("../test-fixtures/tone.ogg");

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kn-sc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn old_settings_files_load_with_sane_defaults() {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Probe {
            providers: ProviderSounds,
        }
        let probe: Probe =
            serde_json::from_str(r#"{"enabled":true,"sound_path":"C:\\x.wav"}"#).unwrap();
        for p in Provider::ALL {
            let cfg = probe.providers.get(p);
            assert!(
                cfg.enabled
                    && cfg.volume == 100
                    && cfg.main.file.is_empty()
                    && cfg.subagent.file.is_empty()
            );
        }
        // partially specified provider keeps unspecified fields at their defaults
        let partial: ProviderSounds = serde_json::from_str(r#"{"codex":{"volume":40}}"#).unwrap();
        assert_eq!(partial.codex.volume, 40);
        assert!(partial.codex.enabled && partial.claude.volume == 100);
    }

    #[test]
    fn import_copies_validates_and_deduplicates() {
        let src = temp("imp-src");
        let dir = temp("imp-store");
        for (bytes, name) in [(WAV, "a.wav"), (MP3, "b.MP3"), (OGG, "c.ogg")] {
            fs::write(src.join(name), bytes).unwrap();
            let choice = import(&src.join(name), &dir).unwrap();
            assert!(is_store_name(&choice.file), "{}", choice.file);
            assert_eq!(choice.name, name);
            assert!(dir.join(&choice.file).is_file());
        }
        fs::copy(src.join("a.wav"), src.join("copy.wav")).unwrap();
        let first = import(&src.join("a.wav"), &dir).unwrap();
        let second = import(&src.join("copy.wav"), &dir).unwrap();
        assert_eq!(first.file, second.file, "identical content is stored once");
        assert!(fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_or_deleting_the_original_does_not_matter() {
        let src = temp("move-src");
        let dir = temp("move-store");
        fs::write(src.join("chime.wav"), WAV).unwrap();
        let choice = import(&src.join("chime.wav"), &dir).unwrap();
        fs::remove_dir_all(&src).unwrap();
        let path = resolve(&dir, &choice).unwrap();
        assert!(
            audio::decode_file(&path).is_ok(),
            "the private copy still plays"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_bad_input() {
        let src = temp("bad-src");
        let dir = temp("bad-store");
        fs::write(src.join("fake.mp3"), b"definitely not an mp3").unwrap();
        fs::write(src.join("prog.exe"), WAV).unwrap();
        assert_eq!(
            import(&src.join("fake.mp3"), &dir).err().as_deref(),
            Some("E_SOUND_DECODE")
        );
        assert_eq!(
            import(&src.join("prog.exe"), &dir).err().as_deref(),
            Some("E_SOUND_INVALID")
        );
        assert_eq!(
            import(&src.join("missing.wav"), &dir).err().as_deref(),
            Some("E_SOUND_MISSING")
        );
        assert_eq!(
            import(&src, &dir).err().as_deref(),
            Some("E_SOUND_INVALID"),
            "a directory is not a sound"
        );
        assert!(
            fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) == 0,
            "nothing is stored on failure"
        );
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_names_cannot_escape_the_folder() {
        assert!(is_store_name("0123456789abcdef.wav"));
        for bad in [
            "../0123456789abcdef.wav",
            "C:/x.wav",
            "0123456789abcdef.exe",
            "x.wav",
            "0123456789ABCDEF.wav",
            "0123456789abcdef.wav\\..\\x",
            "",
            ".wav",
            "/etc/passwd",
        ] {
            assert!(!is_store_name(bad), "{bad}");
        }
        let dir = Path::new("data");
        let evil = SoundChoice {
            file: "../secret.wav".into(),
            name: "x".into(),
        };
        assert_eq!(
            resolve(dir, &evil),
            None,
            "a tampered name falls back to the default"
        );
        assert_eq!(resolve(dir, &SoundChoice::default()), None);
    }

    #[test]
    fn cleanup_only_removes_unreferenced_store_files() {
        let dir = temp("clean");
        fs::write(dir.join("0000000000000001.wav"), WAV).unwrap();
        fs::write(dir.join("0000000000000002.mp3"), MP3).unwrap();
        fs::write(dir.join("notes.txt"), b"mine").unwrap();
        let mut sounds = ProviderSounds::default();
        sounds.claude.main.file = "0000000000000001.wav".into();
        cleanup(&dir, &sounds);
        assert!(dir.join("0000000000000001.wav").is_file());
        assert!(!dir.join("0000000000000002.mp3").exists());
        assert!(
            dir.join("notes.txt").is_file(),
            "foreign files are never touched"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_keeps_the_old_custom_sound_for_every_provider() {
        let src = temp("mig-src");
        let dir = temp("mig-store");
        fs::write(src.join("my sound.wav"), WAV).unwrap();
        let mut sounds = ProviderSounds::default();
        let mut version = 0;
        assert!(migrate(
            src.join("my sound.wav").to_str().unwrap(),
            &mut version,
            &mut sounds,
            &dir
        ));
        assert_eq!(version, SOUNDS_VERSION);
        let expected = sounds.claude.main.clone();
        assert!(!expected.file.is_empty() && expected.name == "my sound.wav");
        assert_eq!(sounds.codex.main, expected);
        assert_eq!(sounds.antigravity.main, expected);
        assert!(
            sounds.claude.subagent.file.is_empty(),
            "sub-agent sound stays the default"
        );
        assert!(sounds.claude.enabled && sounds.claude.volume == 100);
        // running it again changes nothing
        let snapshot = sounds.clone();
        assert!(!migrate("ignored", &mut version, &mut sounds, &dir));
        assert_eq!(sounds, snapshot);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_with_missing_legacy_file_or_default_sound_is_harmless() {
        let dir = temp("mig2");
        let mut sounds = ProviderSounds::default();
        let mut version = 0;
        assert!(migrate(
            "Z:/gone/forever.wav",
            &mut version,
            &mut sounds,
            &dir
        ));
        assert_eq!(sounds, ProviderSounds::default());
        let mut version = 0;
        assert!(migrate("", &mut version, &mut sounds, &dir));
        assert_eq!(sounds, ProviderSounds::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn providers_are_independent() {
        let mut sounds = ProviderSounds::default();
        sounds.get_mut(Provider::Claude).volume = 10;
        sounds.get_mut(Provider::Codex).enabled = false;
        sounds.get_mut(Provider::Antigravity).main.file = "0000000000000009.ogg".into();
        assert_eq!(sounds.get(Provider::Codex).volume, 100);
        assert!(sounds.get(Provider::Claude).enabled);
        assert!(sounds.get(Provider::Claude).main.file.is_empty());
        assert_eq!(sounds.get(Provider::Antigravity).volume, 100);
    }
}
