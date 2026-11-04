// SPDX-License-Identifier: GPL-2.0-or-later
//! The command line and working directory of a launch (`launch.md` §3).

use std::path::{Path, PathBuf};

/// A path from a launch option that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the launch option's path {0:?} is not a relative path inside the install directory")]
pub struct BadPath(pub String);

/// `relative` (with `/` or `\` separators) under `base`. Empty
/// components and `.` are dropped; `..`, a drive or a root is refused, so
/// the result never leaves `base`.
pub fn under(base: &Path, relative: &str) -> Result<PathBuf, BadPath> {
    let bad = || BadPath(relative.to_owned());
    let mut path = base.to_path_buf();
    let mut any = false;
    if relative.starts_with(['/', '\\']) {
        return Err(bad());
    }
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return Err(bad()),
            p if p.contains(':') => return Err(bad()),
            p => {
                path.push(p);
                any = true;
            }
        }
    }
    if any { Ok(path) } else { Err(bad()) }
}

/// `path`, an existing file or directory found without regard to case
/// below `base`, as Windows and Wine find it (`launch.md` §2): each
/// component after `base` that does not exist as written is replaced by
/// the first entry of its directory that matches it without regard to
/// case. `None` when a component has no match.
pub fn find_ignoring_case(base: &Path, path: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(base).ok()?;
    let mut found = base.to_path_buf();
    for part in rest.components() {
        let name = part.as_os_str();
        let exact = found.join(name);
        if exact.symlink_metadata().is_ok() {
            found = exact;
            continue;
        }
        let want = name.to_str()?.to_lowercase();
        let mut entries: Vec<_> = std::fs::read_dir(&found)
            .ok()?
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.to_str().is_some_and(|n| n.to_lowercase() == want))
            .collect();
        entries.sort();
        found.push(entries.first()?);
    }
    Some(found)
}

/// The working directory: `working_dir` under the install directory, or
/// the install directory itself (`launch.md` §3).
pub fn working_dir(install_dir: &Path, working_dir: &str) -> Result<PathBuf, BadPath> {
    if working_dir.trim().is_empty() {
        Ok(install_dir.to_path_buf())
    } else {
        under(install_dir, working_dir)
    }
}

/// The command line `/bin/sh -c` runs: the executable quoted, the
/// option's arguments as written, the extra arguments quoted.
pub fn shell_command_line(executable: &Path, arguments: &str, extra: &[String]) -> String {
    let mut line = shell_quote(&executable.to_string_lossy());
    let arguments = arguments.trim();
    if !arguments.is_empty() {
        line.push(' ');
        line.push_str(arguments);
    }
    for a in extra {
        line.push(' ');
        line.push_str(&shell_quote(a));
    }
    line
}

/// `s` in single quotes, with each `'` written `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command line for `CreateProcessW`: the executable in double
/// quotes, the option's arguments as written, the extra arguments quoted
/// so that `CommandLineToArgvW` gives them back.
pub fn windows_command_line(executable: &Path, arguments: &str, extra: &[String]) -> String {
    // The program name is parsed differently from the arguments: it ends
    // at the next quote, and backslashes are not escapes, so it is only
    // wrapped in quotes. A path cannot contain a quote.
    let mut line = format!("\"{}\"", executable.display());
    let arguments = arguments.trim();
    if !arguments.is_empty() {
        line.push(' ');
        line.push_str(arguments);
    }
    for a in extra {
        line.push(' ');
        windows_quote_into(a, &mut line);
    }
    line
}

fn windows_quote_into(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Each backslash before a quote is doubled, and the quote
                // is escaped.
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote are doubled as well.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

#[cfg(test)]
mod tests {

    #[test]
    fn names_are_found_without_regard_to_case() {
        let base = std::env::temp_dir().join(format!("ossteam-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Bin")).unwrap();
        std::fs::write(base.join("Bin/SteamworksExample.exe"), b"").unwrap();
        let want = under(&base, "bin\\SteamWorksExample.exe").unwrap();
        assert_eq!(
            find_ignoring_case(&base, &want),
            Some(base.join("Bin/SteamworksExample.exe"))
        );
        assert_eq!(find_ignoring_case(&base, &base.join("bin/other.exe")), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    use super::*;

    #[test]
    fn paths_stay_under_the_base() {
        let base = Path::new("/lib/apps/Game");
        assert_eq!(
            under(base, "bin\\game.exe").unwrap(),
            base.join("bin").join("game.exe")
        );
        assert_eq!(
            under(base, "./bin//run.sh").unwrap(),
            base.join("bin").join("run.sh")
        );
        for bad in [
            "",
            ".",
            "../x",
            "bin/../../x",
            "/etc/passwd",
            "\\x",
            "C:\\x",
            "c:x",
        ] {
            assert!(under(base, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn working_directory() {
        let base = Path::new("/g");
        assert_eq!(working_dir(base, "").unwrap(), Path::new("/g"));
        assert_eq!(working_dir(base, "data").unwrap(), Path::new("/g/data"));
    }

    #[test]
    fn shell_line_quotes_what_the_official_client_quotes() {
        let line = shell_command_line(
            Path::new("/g/Some Game/run.sh"),
            " -steam -game \"x y\" ",
            &["+map".into(), "it's".into()],
        );
        assert_eq!(
            line,
            r#"'/g/Some Game/run.sh' -steam -game "x y" '+map' 'it'\''s'"#
        );
        assert_eq!(shell_command_line(Path::new("/g/a"), "", &[]), "'/g/a'");
    }

    #[test]
    fn windows_line_round_trips_extra_arguments() {
        let line = windows_command_line(
            Path::new(r"C:\Games\Game\game.exe"),
            "-dx11",
            &[
                "plain".into(),
                "two words".into(),
                r#"a"b"#.into(),
                r"trailing\".into(),
                r"C:\dir with space\".into(),
                String::new(),
            ],
        );
        assert_eq!(
            line,
            r#""C:\Games\Game\game.exe" -dx11 plain "two words" "a\"b" trailing\ "C:\dir with space\\" """#
        );
    }
}
