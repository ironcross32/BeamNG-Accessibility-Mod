//! The window half of the developer console (`BeamTelFrame.on_console_message`,
//! `_rebuild_context_choice`, `_console_append`): one record from consoleAccessible.lua in,
//! what the window does with it out.
//!
//! The parser is pure, so it is tested without a window. The shell turns each [`Action`]
//! into a `console-line` / `console-contexts` event or a `say()`.

use beam_base::pyfmt::py_strip;
use beam_base::pyval::py_int_parse;

/// What one record asks the window to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `_console_append(line)`: a `console-line` event (trailing newlines stripped).
    Line(String),
    /// `_rebuild_context_choice()`: a `console-contexts` event.
    Contexts(Vec<(i64, String)>),
    /// The single-line result of a command, spoken on `EXECEND` (`say(lines[0].strip())`).
    Speak(String),
}

/// `_console_ctx_pending` and `_console_cmd_lines`.
#[derive(Debug, Default)]
pub struct ConsoleView {
    ctx_pending: Vec<(i64, String)>,
    cmd_lines: Option<Vec<String>>,
}

fn line(text: String) -> Action {
    Action::Line(text.trim_end_matches('\n').to_string())
}

/// `text[2:]` by characters.
fn drop_two(s: &str) -> &str {
    let mut it = s.char_indices();
    match it.nth(2) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

impl ConsoleView {
    pub fn new() -> Self {
        Self::default()
    }

    /// `on_console_message(text)`.
    pub fn handle(&mut self, text: &str) -> Vec<Action> {
        let parts: Vec<&str> = text.split('|').collect();
        let tag = parts[0];
        let mut out = Vec::new();
        match tag {
            "CTX" => {
                if parts.len() >= 3 {
                    let Some(idx) = py_int_parse(parts[1]).and_then(|n| n.to_i64()) else {
                        return out;
                    };
                    self.ctx_pending.push((idx, parts[2..].join("|")));
                }
            }
            "CTXEND" => {
                if !self.ctx_pending.is_empty() {
                    out.push(Action::Contexts(std::mem::take(&mut self.ctx_pending)));
                }
            }
            "RESP" => {
                let status = parts.get(1).copied().unwrap_or("");
                let body = if parts.len() > 2 {
                    parts[2..].join("|")
                } else {
                    String::new()
                };
                // A RESP marks the start of one command's response; capture its output so
                // a single-line result can be spoken on EXECEND. Log-stream lines arrive
                // outside this window and are never captured or spoken.
                let mut cmd = Vec::new();
                match status {
                    "error" => {
                        out.push(line(format!("  error: {body}")));
                        cmd.push(if body.is_empty() {
                            "error".to_string()
                        } else {
                            format!("error: {body}")
                        });
                    }
                    "ok" => {
                        if !body.is_empty() {
                            out.push(line(format!("  = {body}")));
                            cmd.push(body);
                        }
                    }
                    "queued" => {
                        out.push(line("  (queued)".to_string()));
                        cmd.push("queued".to_string());
                    }
                    _ => {}
                }
                self.cmd_lines = Some(cmd);
            }
            "OUT" => {
                let content = parts[1..].join("|");
                out.push(line(format!("  {content}")));
                if let Some(cmd) = self.cmd_lines.as_mut() {
                    let spoken = if content.starts_with("= ") {
                        drop_two(&content).to_string()
                    } else {
                        content
                    };
                    cmd.push(spoken);
                }
            }
            "EXECEND" => {
                // Speak the result only if the whole command produced exactly one line.
                if let Some(cmd) = self.cmd_lines.take() {
                    let lines: Vec<&String> =
                        cmd.iter().filter(|l| !py_strip(l).is_empty()).collect();
                    if lines.len() == 1 {
                        out.push(Action::Speak(py_strip(lines[0]).to_string()));
                    }
                }
            }
            "LOG" => {
                let (lvl, origin, msg) = if parts.len() >= 5 {
                    (parts[2].to_string(), parts[3].to_string(), parts[4..].join("|"))
                } else {
                    (String::new(), String::new(), parts[1..].join("|"))
                };
                out.push(line(format!("  [{lvl} {origin}] {msg}")));
            }
            _ => {}
        }
        out
    }
}
