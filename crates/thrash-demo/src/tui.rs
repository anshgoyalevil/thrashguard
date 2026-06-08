//! Tiny dependency-free ANSI helpers for the demo's terminal output.

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";

pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const CYAN: &str = "\x1b[36m";
pub const GREY: &str = "\x1b[90m";
pub const RED_BG: &str = "\x1b[41m\x1b[97m";

pub fn paint(color: &str, s: &str) -> String {
    format!("{color}{s}{RESET}")
}

pub fn bold(s: &str) -> String {
    format!("{BOLD}{s}{RESET}")
}

pub fn dim(s: &str) -> String {
    format!("{DIM}{s}{RESET}")
}

/// Draw a boxed, wrapped block — used to render the injected intervention.
pub fn boxed(title: &str, title_color: &str, body: &str, width: usize) -> String {
    let inner = width.saturating_sub(2);
    let mut out = String::new();
    out.push_str(&format!("{title_color}╔{}╗{RESET}\n", "═".repeat(inner)));
    let titled = format!(" {title} ");
    let pad = inner.saturating_sub(visible_len(&titled));
    out.push_str(&format!(
        "{title_color}║{RESET}{}{}{title_color}║{RESET}\n",
        bold(&titled),
        " ".repeat(pad)
    ));
    out.push_str(&format!("{title_color}╟{}╢{RESET}\n", "─".repeat(inner)));
    for line in wrap(body, inner.saturating_sub(2)) {
        let pad = inner.saturating_sub(visible_len(&line) + 1);
        out.push_str(&format!(
            "{title_color}║{RESET} {line}{}{title_color}║{RESET}\n",
            " ".repeat(pad)
        ));
    }
    out.push_str(&format!("{title_color}╚{}╝{RESET}", "═".repeat(inner)));
    out
}

/// Visible length ignoring ANSI escape sequences.
fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            len += 1;
        }
    }
    len
}

/// Word-wrap to `width` columns (whitespace-naive, good enough for prose).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}
