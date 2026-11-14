// Store module: application-owned persistence (SQLite + settings file).
// Rule: this module never holds conversation transcripts — pi owns session files;
// nabla.db only indexes them (see .docs/02-decisiones.md).
// Rule: workspaces/sessions live in SQLite; user-editable preferences live in settings.json.

use tauri::command;

#[command]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_includes_name() {
        let out = greet("nabla");
        assert!(out.contains("nabla"));
    }
}
