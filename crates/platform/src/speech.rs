//! Speaking a word through the system voice.
//!
//! There is no cross-platform text-to-speech in the standard library, and
//! pulling in a speech engine is not worth it for pronouncing one word. Each
//! platform already ships a voice, so this shells out to it:
//!
//! * Windows — SAPI through `System.Speech`, reached with PowerShell.
//! * macOS — `say`.
//! * Linux — `spd-say`, falling back to `espeak`.
//!
//! Speech is intentionally asynchronous: `speak` spawns and returns, because
//! pronouncing a word must never stall the frame that drew the popup. Starting
//! a new word stops whatever is still being said, matching the single
//! Speak/Stop button the popup shows.

use std::io::Error as IoError;
use std::process::{Child, Command};

/// Why a word could not be spoken.
#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// No usable voice was found on this system.
    #[error("no text-to-speech voice is available ({0})")]
    Unavailable(&'static str),
    /// The command was found but could not be started.
    #[error("could not start the speech command: {0}")]
    Spawn(#[from] IoError),
}

/// Owns at most one in-flight utterance.
///
/// Create one per place that can speak — the lookup popup keeps one in its own
/// state — and call [`Self::speak`] to say something, which cancels whatever is
/// still playing first.
#[derive(Debug, Default)]
pub struct Speaker {
    /// The running speech command, if one is still alive.
    child: Option<Child>,
}

impl Speaker {
    /// A speaker with nothing playing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Say `text`, stopping anything already playing.
    ///
    /// `lang` is a BCP-47 prefix such as `en` or `bn`, used to pick a matching
    /// system voice when one is installed. It is a hint: a system with only an
    /// English voice still speaks, badly, rather than not at all.
    pub fn speak(&mut self, text: &str, lang: &str) -> Result<(), SpeechError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        self.stop();

        let mut command = speech_command(text, lang)?;
        // Keep the child from flashing a console window (Windows).
        hide_console(&mut command);
        // Detached and silent: a hung voice must never block the application
        // from closing, and with no console attached the child must not try to
        // write to one.
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        self.child = Some(command.spawn()?);
        Ok(())
    }

    /// Stop speaking, if anything is playing.
    pub fn stop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            // Ignore the result: a voice that already finished is not an error.
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
    }

    /// Whether a voice is still running.
    pub fn is_speaking(&mut self) -> bool {
        match self.child.as_mut() {
            None => false,
            Some(child) => matches!(child.try_wait(), Ok(None)),
        }
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Stop Windows from giving the child a console window of its own.
///
/// The app is a GUI process (`windows_subsystem = "windows"`), so spawning a
/// console program such as PowerShell allocates a brand-new console window that
/// flashes on screen for every utterance — very visible with auto-speak on.
/// `CREATE_NO_WINDOW` tells Windows not to allocate one at all; unlike
/// `-WindowStyle Hidden`, which only hides the window after it has appeared.
#[cfg(target_os = "windows")]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW` from `winbase.h`.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// No-op on platforms where the speech helper is not a console program.
#[cfg(not(target_os = "windows"))]
fn hide_console(_command: &mut Command) {}

/// Build the platform command that speaks `text`.
// The signature matches the Linux variant, which can fail when no voice is
// installed; Windows PowerShell is always present, so this arm never errors.
#[cfg(target_os = "windows")]
#[allow(clippy::unnecessary_wraps)]
fn speech_command(text: &str, lang: &str) -> Result<Command, SpeechError> {
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         foreach ($v in $s.GetInstalledVoices()) {{ \
           if ($v.VoiceInfo.Culture.Name -like '{lang}*') {{ $s.SelectVoice($v.VoiceInfo.Name); break }} \
         }}; \
         $s.Speak('{text}')",
        lang = shell_lang(lang),
        text = powershell_quote(text),
    );
    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &script,
    ]);
    Ok(command)
}

/// Build the platform command that speaks `text`.
// `say` is always present on macOS, so this arm never errors; the Result
// matches the Linux variant, which can fail when no voice is installed.
#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)]
fn speech_command(text: &str, lang: &str) -> Result<Command, SpeechError> {
    let mut command = Command::new("say");
    let voice = shell_lang(lang);
    if !voice.is_empty() {
        command.args(["-v", &voice]);
    }
    command.arg(text);
    Ok(command)
}

/// Build the platform command that speaks `text`.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn speech_command(text: &str, lang: &str) -> Result<Command, SpeechError> {
    // speech-dispatcher is the usual desktop option; espeak is the fallback
    // that is present on minimal setups.
    for (program, flag) in [("spd-say", "-l"), ("espeak", "-v")] {
        if which(program) {
            let mut command = Command::new(program);
            command.args([flag, shell_lang(lang).as_str(), "--", text]);
            return Ok(command);
        }
    }
    Err(SpeechError::Unavailable("install spd-say or espeak"))
}

/// Whether a program is on `PATH`.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn which(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Reduce a language tag to its alphabetic prefix, `bn-IN` to `bn`.
///
/// Removing anything that is not a letter keeps the value safe to interpolate
/// into a shell command.
fn shell_lang(lang: &str) -> String {
    lang.chars()
        .take_while(char::is_ascii_alphabetic)
        .filter(char::is_ascii_alphabetic)
        .collect()
}

/// Quote text for a PowerShell single-quoted string.
///
/// The only escape a single-quoted PowerShell string needs is a doubled
/// apostrophe, which is what stops a word like `don't` from ending the string
/// and running the rest as script.
fn powershell_quote(text: &str) -> String {
    text.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_reduced_to_letters() {
        assert_eq!(shell_lang("bn-IN"), "bn");
        assert_eq!(shell_lang("en"), "en");
        assert_eq!(shell_lang("; rm -rf /"), "");
    }

    #[test]
    fn apostrophes_are_doubled_not_left_to_break_the_script() {
        assert_eq!(powershell_quote("don't"), "don''t");
        assert_eq!(powershell_quote("plain"), "plain");
    }

    #[test]
    fn a_new_speaker_is_silent() {
        let mut speaker = Speaker::new();
        assert!(!speaker.is_speaking());
        // Stopping when nothing plays is harmless.
        speaker.stop();
    }

    #[test]
    fn empty_text_says_nothing() {
        let mut speaker = Speaker::new();
        assert!(speaker.speak("   ", "en").is_ok());
        assert!(!speaker.is_speaking());
    }
}
