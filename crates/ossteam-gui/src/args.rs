// SPDX-License-Identifier: GPL-2.0-or-later
//! Splitting the App page's "Arguments" field into `extra_args` the way a
//! POSIX shell splits words, without any expansion (`gui.md` §4.3).

/// The words of `s`. Single quotes keep everything literally; inside
/// double quotes a backslash escapes only `"`, `\`, `$` and `` ` ``;
/// elsewhere it escapes any character. An unterminated quote or a
/// trailing backslash is an error.
pub fn split(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("a single quote is not closed".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("a double quote is not closed".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("a double quote is not closed".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('\n') => {}
                    Some(c) => word.push(c),
                    None => return Err("the arguments end with a backslash".into()),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn splits_like_a_shell_without_expanding() {
        assert_eq!(split("").unwrap(), Vec::<String>::new());
        assert_eq!(
            split("  -novid   -w 1920 ").unwrap(),
            ["-novid", "-w", "1920"]
        );
        assert_eq!(
            split(r#"+name "Some Player" 'a b'"#).unwrap(),
            ["+name", "Some Player", "a b"]
        );
        assert_eq!(split(r#"a"b"'c'd"#).unwrap(), ["abcd"]);
        assert_eq!(split(r#""""#).unwrap(), [""]);
        assert_eq!(split(r"a\ b").unwrap(), ["a b"]);
        assert_eq!(split(r#""\$HOME \x \"q\"""#).unwrap(), [r#"$HOME \x "q""#]);
        assert_eq!(split("'$HOME' ~ *").unwrap(), ["$HOME", "~", "*"]);
    }

    #[test]
    fn unfinished_quoting_is_an_error() {
        assert!(split("'open").is_err());
        assert!(split("\"open").is_err());
        assert!(split("end\\").is_err());
    }
}
