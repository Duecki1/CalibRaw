//! Edit In on desktops: the editors installed here and how a file is opened in
//! one. An editor's program is a Linux `.desktop` entry, a macOS `.app`
//! bundle, or a program started with the file as its argument.

use crate::edit_in::ExternalEditor;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::i18n::tr;
use crate::platform::{EditInSupport, FileDialog};
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) fn support() -> EditInSupport {
    // A Flatpak sandbox cannot start host programs; its portal asks instead.
    if super::portal::is_flatpak() {
        EditInSupport::SystemChooser
    } else {
        EditInSupport::Editors
    }
}

pub(super) fn installed() -> Vec<ExternalEditor> {
    #[cfg(target_os = "linux")]
    {
        desktop_entry::installed_editors()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

pub(super) fn program_dialog() -> FileDialog {
    #[cfg(target_os = "windows")]
    {
        let dialog = FileDialog::new().add_filter(tr("Programs"), &["exe"]);
        match std::env::var_os("ProgramFiles") {
            Some(folder) => dialog.set_directory(folder),
            None => dialog,
        }
    }
    #[cfg(target_os = "macos")]
    {
        FileDialog::new()
            .add_filter(tr("Applications"), &["app"])
            .set_directory("/Applications")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Programs and their `.desktop` entries are both accepted.
        FileDialog::new().set_directory("/usr/share/applications")
    }
}

pub(super) fn editor_for_program(program: &Path) -> Option<ExternalEditor> {
    #[cfg(target_os = "linux")]
    if desktop_entry::is_entry(program) {
        let entry = desktop_entry::read(program).ok()?;
        return entry.opens_files().then(|| ExternalEditor {
            name: entry.name,
            program: program.to_path_buf(),
            send_original: false,
        });
    }
    // A macOS `.app` is a folder; anything else must be a file to run.
    let runnable = if cfg!(target_os = "macos") && has_extension(program, "app") {
        program.is_dir()
    } else {
        program.is_file()
    };
    let name = program.file_stem()?.to_string_lossy().into_owned();
    runnable.then(|| ExternalEditor {
        name,
        program: program.to_path_buf(),
        send_original: false,
    })
}

/// Opens `file` in `program`, or through the portal's chooser when `None`.
pub(super) fn open(program: Option<&Path>, file: &Path) -> Result<(), String> {
    let Some(program) = program else {
        return open_with_chooser(file);
    };
    let mut command = command(program, file)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    // Reaps the editor when it exits, so it does not stay behind as a zombie
    // while CalibRaw runs. The editor outlives CalibRaw if CalibRaw closes first.
    if let Err(error) = std::thread::Builder::new()
        .name("calibraw-edit-in-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
        })
    {
        log::warn!("could not watch the editor process: {error}");
    }
    Ok(())
}

fn command(program: &Path, file: &Path) -> Result<Command, String> {
    #[cfg(target_os = "linux")]
    if desktop_entry::is_entry(program) {
        let entry = desktop_entry::read(program)?;
        return entry.command(program, file);
    }
    if cfg!(target_os = "macos") && has_extension(program, "app") {
        let mut command = Command::new("open");
        command.arg("-a").arg(program).arg(file);
        return Ok(command);
    }
    let mut command = Command::new(program);
    command.arg(file);
    Ok(command)
}

fn open_with_chooser(file: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        // The portal call returns once the chooser is up; run it off the UI
        // thread, and report only a failure that comes quickly.
        let file = file.to_path_buf();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("calibraw-edit-in-portal".to_owned())
            .spawn(move || {
                let result = super::portal::open_file_with_chooser(&file)
                    .map_err(|error| format!("{error:#}"));
                if let Err(error) = &result {
                    log::error!("Could not open {} for editing: {error}", file.display());
                }
                let _ = sender.send(result);
            })
            .map_err(|error| error.to_string())?;
        receiver
            .recv_timeout(std::time::Duration::from_millis(500))
            .unwrap_or(Ok(()))
    }
    // Only a Flatpak sandbox asks through a chooser (`support`).
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        Err("an application chooser is not available on this platform".to_owned())
    }
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(extension))
}

/// Freedesktop Desktop Entries (Desktop Entry Specification 1.5): finding the
/// applications that open TIFF files, and running one with a file.
#[cfg(target_os = "linux")]
mod desktop_entry {
    use super::has_extension;
    use crate::edit_in::ExternalEditor;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Entries larger than this are not application launchers.
    const MAX_ENTRY_BYTES: u64 = 256 * 1024;
    /// Bounds the walk of the application folders.
    const MAX_ENTRIES: usize = 10_000;
    const MAX_DEPTH: usize = 4;
    /// The editors Edit In offers handle what it writes.
    const EDITED_MIME_TYPE: &str = "image/tiff";

    pub(super) struct DesktopEntry {
        pub(super) name: String,
        exec: String,
        icon: Option<String>,
        working_directory: Option<PathBuf>,
        mime_types: Vec<String>,
        application: bool,
        hidden: bool,
        terminal: bool,
    }

    impl DesktopEntry {
        /// Whether it is a graphical application that can be given a file.
        pub(super) fn opens_files(&self) -> bool {
            self.application && !self.terminal && !self.exec.trim().is_empty()
        }

        /// Its `Exec` line with `file` in place of the field code, or after
        /// the arguments when the line has none.
        pub(super) fn command(&self, entry: &Path, file: &Path) -> Result<Command, String> {
            let mut arguments = Vec::new();
            let mut gave_file = false;
            for token in split_exec(&self.exec)? {
                match token.as_str() {
                    "%f" | "%F" | "%u" | "%U" => {
                        arguments.push(file.as_os_str().to_owned());
                        gave_file = true;
                    }
                    "%i" => {
                        if let Some(icon) = &self.icon {
                            arguments.push("--icon".into());
                            arguments.push(icon.into());
                        }
                    }
                    // Deprecated field codes expand to nothing.
                    "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
                    _ => arguments.push(self.expand_inline(&token, entry, file, &mut gave_file)),
                }
            }
            if !gave_file {
                arguments.push(file.as_os_str().to_owned());
            }
            let mut arguments = arguments.into_iter();
            let program = arguments
                .next()
                .filter(|program| !program.is_empty())
                .ok_or_else(|| format!("{} has an empty Exec line", entry.display()))?;
            let mut command = Command::new(program);
            command.args(arguments);
            if let Some(directory) = &self.working_directory {
                command.current_dir(directory);
            }
            Ok(command)
        }

        /// Field codes inside a longer argument, such as `--file=%f`.
        fn expand_inline(
            &self,
            token: &str,
            entry: &Path,
            file: &Path,
            gave_file: &mut bool,
        ) -> OsString {
            let mut expanded = OsString::new();
            let mut characters = token.chars();
            while let Some(character) = characters.next() {
                if character != '%' {
                    expanded.push(character.encode_utf8(&mut [0; 4]));
                    continue;
                }
                match characters.next() {
                    Some('f' | 'F' | 'u' | 'U') => {
                        expanded.push(file.as_os_str());
                        *gave_file = true;
                    }
                    Some('c') => expanded.push(&self.name),
                    Some('k') => expanded.push(entry.as_os_str()),
                    Some('%') => expanded.push("%"),
                    // `%i` only stands alone; other codes expand to nothing.
                    Some(_) | None => {}
                }
            }
            expanded
        }
    }

    pub(super) fn is_entry(program: &Path) -> bool {
        has_extension(program, "desktop")
    }

    pub(super) fn read(path: &Path) -> Result<DesktopEntry, String> {
        let metadata =
            std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if metadata.len() > MAX_ENTRY_BYTES {
            return Err(format!(
                "{} is too large for a desktop entry",
                path.display()
            ));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(parse(&text))
    }

    /// The `[Desktop Entry]` group's keys this needs. Localized keys
    /// (`Name[de]`) are not read: the editor list shows the untranslated name.
    fn parse(text: &str) -> DesktopEntry {
        let mut entry = DesktopEntry {
            name: String::new(),
            exec: String::new(),
            icon: None,
            working_directory: None,
            mime_types: Vec::new(),
            application: false,
            hidden: false,
            terminal: false,
        };
        let mut in_main_group = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_main_group = line == "[Desktop Entry]";
                continue;
            }
            if !in_main_group {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unescape_value(value.trim());
            match key.trim() {
                "Type" => entry.application = value == "Application",
                "Name" => entry.name = value,
                "Exec" => entry.exec = value,
                "Icon" if !value.is_empty() => entry.icon = Some(value),
                "Path" if !value.is_empty() => entry.working_directory = Some(value.into()),
                "MimeType" => {
                    entry.mime_types = value
                        .split(';')
                        .filter(|mime| !mime.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                // `Hidden` deletes the entry. `NoDisplay` only keeps it out of
                // menus: Krita, for one, opens TIFF files through such an
                // entry (`krita_tiff.desktop`), which "Open with" lists.
                "Hidden" => entry.hidden = value == "true",
                "Terminal" => entry.terminal = value == "true",
                _ => {}
            }
        }
        entry
    }

    /// The escapes of string values: `\s`, `\n`, `\t`, `\r` and `\\`.
    fn unescape_value(value: &str) -> String {
        let mut unescaped = String::with_capacity(value.len());
        let mut characters = value.chars();
        while let Some(character) = characters.next() {
            if character != '\\' {
                unescaped.push(character);
                continue;
            }
            match characters.next() {
                Some('s') => unescaped.push(' '),
                Some('n') => unescaped.push('\n'),
                Some('t') => unescaped.push('\t'),
                Some('r') => unescaped.push('\r'),
                Some('\\') => unescaped.push('\\'),
                // Left for the Exec quoting rules, such as `\"` in a quoted argument.
                Some(other) => {
                    unescaped.push('\\');
                    unescaped.push(other);
                }
                None => unescaped.push('\\'),
            }
        }
        unescaped
    }

    /// Splits an `Exec` value into arguments: separated by spaces, or quoted
    /// in double quotes, inside which `\"`, `` \` ``, `\$` and `\\` stand
    /// for the character.
    fn split_exec(exec: &str) -> Result<Vec<String>, String> {
        let mut arguments = Vec::new();
        let mut current = String::new();
        let mut in_argument = false;
        let mut quoted = false;
        let mut characters = exec.chars();
        while let Some(character) = characters.next() {
            match character {
                '"' => {
                    quoted = !quoted;
                    in_argument = true;
                }
                '\\' if quoted => match characters.next() {
                    Some(escaped @ ('"' | '`' | '$' | '\\')) => current.push(escaped),
                    Some(other) => {
                        current.push('\\');
                        current.push(other);
                    }
                    None => current.push('\\'),
                },
                ' ' | '\t' if !quoted => {
                    if in_argument {
                        arguments.push(std::mem::take(&mut current));
                        in_argument = false;
                    }
                }
                _ => {
                    current.push(character);
                    in_argument = true;
                }
            }
        }
        if quoted {
            return Err(format!("unterminated quote in Exec line `{exec}`"));
        }
        if in_argument {
            arguments.push(current);
        }
        Ok(arguments)
    }

    /// The `applications` folders, most important first: the user's data
    /// folder, then the system's, then Flatpak's exports where the session
    /// does not list them already.
    fn application_folders() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
        let data_dirs = std::env::var_os("XDG_DATA_DIRS")
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let mut folders: Vec<PathBuf> = data_home.into_iter().collect();
        folders.extend(std::env::split_paths(&data_dirs).filter(|path| path.is_absolute()));
        folders.extend(home.map(|home| home.join(".local/share/flatpak/exports/share")));
        folders.push(PathBuf::from("/var/lib/flatpak/exports/share"));
        let mut seen = HashSet::new();
        folders
            .into_iter()
            .filter(|folder| seen.insert(folder.clone()))
            .map(|folder| folder.join("applications"))
            .collect()
    }

    /// Applications that open TIFF files. An entry hides any later one with
    /// the same desktop ID, as the specification orders them, also when it
    /// is itself hidden.
    pub(super) fn installed_editors() -> Vec<ExternalEditor> {
        let mut seen_ids = HashSet::new();
        let mut editors = Vec::new();
        let mut visited = 0;
        for folder in application_folders() {
            let mut pending = vec![(folder.clone(), 0)];
            while let Some((directory, depth)) = pending.pop() {
                let Ok(entries) = std::fs::read_dir(&directory) else {
                    continue;
                };
                for entry in entries.flatten() {
                    visited += 1;
                    if visited > MAX_ENTRIES {
                        log::warn!("stopped looking for editors after {MAX_ENTRIES} files");
                        return sorted(editors);
                    }
                    let path = entry.path();
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    if file_type.is_dir() {
                        if depth < MAX_DEPTH {
                            pending.push((path, depth + 1));
                        }
                        continue;
                    }
                    if !is_entry(&path) {
                        continue;
                    }
                    // The desktop ID: the path below `applications`, with `-`
                    // for each `/`.
                    let Ok(relative) = path.strip_prefix(&folder) else {
                        continue;
                    };
                    let id = relative.to_string_lossy().replace('/', "-");
                    if !seen_ids.insert(id.clone()) {
                        continue;
                    }
                    // CalibRaw is not an editor to hand its own images to.
                    if id.to_ascii_lowercase().contains("calibraw") {
                        continue;
                    }
                    let Ok(desktop_entry) = read(&path) else {
                        continue;
                    };
                    if desktop_entry.hidden
                        || !desktop_entry.opens_files()
                        || desktop_entry.name.is_empty()
                        || !desktop_entry
                            .mime_types
                            .iter()
                            .any(|mime| mime.eq_ignore_ascii_case(EDITED_MIME_TYPE))
                    {
                        continue;
                    }
                    editors.push(ExternalEditor {
                        name: desktop_entry.name,
                        program: path,
                        send_original: false,
                    });
                }
            }
        }
        sorted(editors)
    }

    fn sorted(mut editors: Vec<ExternalEditor>) -> Vec<ExternalEditor> {
        editors.sort_by_cached_key(|editor| editor.name.to_lowercase());
        editors
    }
}
