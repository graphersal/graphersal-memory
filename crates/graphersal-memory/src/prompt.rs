//! The questions of the interactive `install` wizard: "Step N of M", a title, a short
//! explanation, numbered choices with the default marked (taken on Enter), and a re-ask on an
//! invalid answer. Plain ASCII layout; colour (bold titles, a dim explanation, a green default)
//! only when the output is a terminal that supports it and `NO_COLOR` is unset. No terminal
//! library: lines on a writer, answers from a reader, so the tests drive it with byte strings.

use std::io::{BufRead, Write};

/// One choice of a question.
#[derive(Debug, Clone)]
pub struct Choice {
    /// The label (first column).
    pub label: String,
    /// What it means (second column; may be empty). Line breaks continue under it.
    pub detail: String,
}

impl Choice {
    /// A choice with a label and a detail.
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
        }
    }
}

/// The width explanation lines are wrapped to (after the two-space indent).
pub const WIDTH: usize = 86;

/// `text` broken into lines of at most `width` characters at spaces (a longer word stays whole).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text` in the ANSI style `code` when `color` is on.
pub fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// An "ACTION NEEDED" line highlighted (bold yellow) when `color` is on; the text says
/// "ACTION NEEDED" either way.
pub fn action_needed(color: bool, text: &str) -> String {
    paint(color, "1;33", text)
}

/// The separator line before a closing block ("Next steps").
pub const SEPARATOR: &str = "----------------------------------------";

/// The answers ran out (the input ended) before a question without a default was answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoAnswer;

/// Whether colour may be used on a terminal, from the environment: `NO_COLOR` unset (any
/// value switches colour off, <https://no-color.org>) and `TERM` not `dumb`.
pub fn color_allowed(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("NO_COLOR").is_none() && env("TERM").is_none_or(|term| term != "dumb")
}

/// The wizard's console: answers from `input`, everything else to `out`.
pub struct Console<'a> {
    input: &'a mut dyn BufRead,
    out: &'a mut dyn Write,
    color: bool,
}

impl<'a> Console<'a> {
    /// A console; `color` switches the ANSI styles on.
    pub fn new(input: &'a mut dyn BufRead, out: &'a mut dyn Write, color: bool) -> Self {
        Self { input, out, color }
    }

    /// The output writer.
    pub fn out(&mut self) -> &mut dyn Write {
        self.out
    }

    fn style(&self, code: &str, text: &str) -> String {
        paint(self.color, code, text)
    }

    /// Bold text (titles).
    pub fn bold(&self, text: &str) -> String {
        self.style("1", text)
    }

    /// Dim text (explanations).
    pub fn dim(&self, text: &str) -> String {
        self.style("2", text)
    }

    /// Green text (the default marker, done).
    pub fn green(&self, text: &str) -> String {
        self.style("32", text)
    }

    /// Yellow text (warnings).
    pub fn yellow(&self, text: &str) -> String {
        self.style("33", text)
    }

    /// The heading of a step and its explanation lines.
    pub fn step(&mut self, n: usize, of: usize, title: &str, explanation: &[&str]) {
        let heading = self.bold(&format!("Step {n} of {of} \u{b7} {title}"));
        let _ = writeln!(self.out, "\n{heading}");
        for line in explanation.iter().flat_map(|line| wrap(line, WIDTH)) {
            let line = self.dim(&line);
            let _ = writeln!(self.out, "  {line}");
        }
        let _ = writeln!(self.out);
    }

    /// Reads one answer line, trimmed; `None` at the end of the input.
    fn answer(&mut self) -> Option<String> {
        let _ = self.out.flush();
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_string()),
        }
    }

    /// Reads one free-text answer line, trimmed; `None` at the end of the input.
    pub fn read_answer(&mut self) -> Option<String> {
        self.answer()
    }

    /// Shows `choices` numbered from 1 and returns the index chosen. `default` (an index) is
    /// marked and taken on Enter (and at the end of the input); without one an answer is
    /// required. An answer that is no number of the list is asked again.
    pub fn choose(
        &mut self,
        choices: &[Choice],
        default: Option<usize>,
    ) -> Result<usize, NoAnswer> {
        let width = choices
            .iter()
            .map(|c| c.label.chars().count())
            .max()
            .unwrap_or(0);
        for (i, choice) in choices.iter().enumerate() {
            let mut lines = choice.detail.lines();
            let first = lines.next().unwrap_or_default();
            let mark = if default == Some(i) {
                format!("  {}", self.green("(default)"))
            } else {
                String::new()
            };
            let line = format!("{:<width$}  {first}", choice.label);
            let _ = writeln!(self.out, "  {}) {}{mark}", i + 1, line.trim_end());
            for more in lines {
                let _ = writeln!(self.out, "     {:<width$}  {more}", "");
            }
        }
        loop {
            let prompt = match default {
                Some(i) => format!("Choose [{}]: ", i + 1),
                None => format!("Choose 1-{}: ", choices.len()),
            };
            let _ = write!(self.out, "{prompt}");
            let Some(answer) = self.answer() else {
                let _ = writeln!(self.out);
                return default.ok_or(NoAnswer);
            };
            if answer.is_empty() {
                if let Some(i) = default {
                    return Ok(i);
                }
            } else if let Ok(n) = answer.parse::<usize>()
                && (1..=choices.len()).contains(&n)
            {
                return Ok(n - 1);
            }
            let again = self.yellow(&format!(
                "  Please type a number from 1 to {}{}.",
                choices.len(),
                if default.is_some() {
                    ", or press Enter for the default"
                } else {
                    ""
                }
            ));
            let _ = writeln!(self.out, "{again}");
        }
    }

    /// A yes/no question as two numbered choices (1 yes, 2 no).
    pub fn yes_no(&mut self, yes: &str, no: &str, default: Option<bool>) -> Result<bool, NoAnswer> {
        let choices = [Choice::new("Yes", yes), Choice::new("No", no)];
        let index = default.map(|yes| if yes { 0 } else { 1 });
        Ok(self.choose(&choices, index)? == 0)
    }

    /// The wizard's last question, `Install? [Y/n, d = show every change]`: Enter (or `y`)
    /// installs; `n` (or the end of the input) does not; `d` shows every change (`details`)
    /// and asks again.
    pub fn confirm_install(&mut self, details: &mut dyn FnMut(&mut dyn Write)) -> bool {
        loop {
            let _ = write!(self.out, "Install? [Y/n, d = show every change]: ");
            let Some(answer) = self.answer() else {
                let _ = writeln!(self.out);
                return false;
            };
            match answer.to_ascii_lowercase().as_str() {
                "" | "y" | "yes" => return true,
                "n" | "no" => return false,
                "d" => {
                    let _ = writeln!(self.out);
                    details(self.out);
                    let _ = writeln!(self.out);
                }
                _ => {
                    let again = self.yellow(
                        "  Please press Enter to install, type n to cancel or d to see every change.",
                    );
                    let _ = writeln!(self.out, "{again}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str, default: Option<usize>) -> (Result<usize, NoAnswer>, String) {
        let mut input = input.as_bytes();
        let mut out = Vec::new();
        let choices = [Choice::new("one", "first\nmore"), Choice::new("two", "")];
        let answer = Console::new(&mut input, &mut out, false).choose(&choices, default);
        (answer, String::from_utf8(out).unwrap())
    }

    #[test]
    fn enter_takes_the_default_and_a_number_a_choice() {
        let (answer, out) = run("\n", Some(1));
        assert_eq!(answer, Ok(1));
        assert!(out.contains("  1) one  first\n"), "{out}");
        assert!(out.contains("\n          more\n"), "{out}");
        assert!(out.contains("  2) two  (default)"), "{out}");
        assert!(out.contains("Choose [2]: "), "{out}");
        assert_eq!(run("1\n", Some(1)).0, Ok(0));
    }

    #[test]
    fn invalid_answers_are_asked_again() {
        let (answer, out) = run("7\nx\n2\n", Some(0));
        assert_eq!(answer, Ok(1));
        assert_eq!(out.matches("Please type a number from 1 to 2").count(), 2);
    }

    #[test]
    fn without_a_default_enter_is_asked_again_and_the_end_is_no_answer() {
        let (answer, out) = run("\n\n2\n", None);
        assert_eq!(answer, Ok(1));
        assert!(out.contains("Choose 1-2: "), "{out}");
        assert_eq!(run("", None).0, Err(NoAnswer));
        assert_eq!(run("", Some(0)).0, Ok(0));
    }

    #[test]
    fn long_explanations_are_wrapped() {
        assert_eq!(wrap("a bb ccc", 4), vec!["a bb", "ccc"]);
        assert_eq!(wrap("", 4), vec![String::new()]);
        assert_eq!(wrap("toolongword x", 4), vec!["toolongword", "x"]);
    }

    #[test]
    fn colour_only_when_allowed() {
        let mut input = &b""[..];
        let mut out = Vec::new();
        let console = Console::new(&mut input, &mut out, true);
        assert_eq!(console.bold("x"), "\x1b[1mx\x1b[0m");
        assert!(color_allowed(&|_| None));
        assert!(!color_allowed(
            &|name| (name == "NO_COLOR").then(String::new)
        ));
        assert!(!color_allowed(
            &|name| (name == "TERM").then(|| "dumb".into())
        ));
    }

    #[test]
    fn d_shows_every_change_and_asks_again() {
        let mut input = &b"d\n\n"[..];
        let mut out = Vec::new();
        let mut shown = 0;
        let installed = Console::new(&mut input, &mut out, false).confirm_install(&mut |out| {
            shown += 1;
            let _ = writeln!(out, "every change");
        });
        assert!(installed);
        assert_eq!(shown, 1);
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out.matches("Install? [Y/n, d = show every change]: ")
                .count(),
            2
        );
        assert!(out.contains("every change"), "{out}");
    }

    #[test]
    fn the_confirmation_applies_on_enter() {
        for (input, expected) in [
            ("\n", true),
            ("y\n", true),
            ("n\n", false),
            ("", false),
            ("?\nn\n", false),
        ] {
            let mut input = input.as_bytes();
            let mut out = Vec::new();
            assert_eq!(
                Console::new(&mut input, &mut out, false).confirm_install(&mut |_| {}),
                expected
            );
        }
    }
}
