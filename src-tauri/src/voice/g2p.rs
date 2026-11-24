//! Grapheme-to-Phoneme (G2P) frontend for Kokoro-82M ONNX.
//!
//! Zero mandatory system dependencies: Spanish uses a built-in pure-Rust IPA
//! rule engine (Spanish orthography is highly phonemic). If `espeak-ng` is
//! installed on the machine, English (and other non-Spanish languages) can
//! optionally leverage `espeak-ng -q --ipa=3`, falling back cleanly to a
//! built-in rule table when absent.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Maximum phoneme token length Kokoro v1.0 style matrices support (510 rows).
pub const KOKORO_MAX_TOKENS: usize = 508;

/// Loads Kokoro's character -> token_id vocabulary from `tokenizer.json`.
pub fn load_kokoro_vocab(tokenizer_path: &Path) -> Result<HashMap<char, i64>, String> {
    let body = std::fs::read_to_string(tokenizer_path)
        .map_err(|e| format!("cannot read Kokoro tokenizer.json: {e}"))?;
    let root: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("invalid Kokoro tokenizer.json: {e}"))?;
    let vocab_obj = root
        .get("model")
        .and_then(|m| m.get("vocab"))
        .and_then(|v| v.as_object())
        .ok_or_else(|| "Kokoro tokenizer.json missing model.vocab".to_string())?;

    let mut map = HashMap::with_capacity(vocab_obj.len());
    for (k, v) in vocab_obj {
        let mut chars = k.chars();
        if let (Some(ch), None, Some(id)) = (chars.next(), chars.next(), v.as_i64()) {
            map.insert(ch, id);
        }
    }
    if map.is_empty() {
        return Err("Kokoro tokenizer.json vocab is empty".to_string());
    }
    Ok(map)
}

/// Converts text into an IPA phoneme string suitable for Kokoro-82M.
pub fn text_to_phonemes(text: &str, language: &str) -> String {
    let lang = language.trim().to_ascii_lowercase();
    let is_spanish = lang.is_empty() || lang.starts_with("es");

    // Try `espeak-ng` first if present on PATH for best prosody across languages.
    let espeak_lang = if is_spanish {
        "es"
    } else if lang == "en-gb" {
        "en-gb"
    } else {
        "en-us"
    };
    if let Some(ipa) = try_espeak_ng(text, espeak_lang) {
        return ipa;
    }

    if is_spanish {
        phonemize_spanish(text)
    } else {
        phonemize_english_fallback(text)
    }
}

fn try_espeak_ng(text: &str, voice: &str) -> Option<String> {
    let mut cmd = Command::new("espeak-ng");
    cmd.args(["-q", "--ipa=3", "-v", voice, text]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        unsafe {
            cmd.creation_flags(crate::process::CREATE_NO_WINDOW);
        }
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let ipa = String::from_utf8(output.stdout).ok()?;
    let cleaned = ipa
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Pure-Rust rule-based Spanish IPA phonemizer (zero external dependencies).
pub fn phonemize_spanish(text: &str) -> String {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut out = String::with_capacity(chars.len() * 2);
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };

        if ch.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            i += 1;
            continue;
        }
        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | '—' | '-') {
            out.push(ch);
            i += 1;
            continue;
        }
        if matches!(ch, '¡' | '¿' | '"' | '\'' | '(' | ')' | '[' | ']') {
            i += 1;
            continue;
        }

        // Digraphs
        if ch == 'c' && next == Some('h') {
            out.push_str("tʃ");
            i += 2;
            continue;
        }
        if ch == 'l' && next == Some('l') {
            out.push('ʝ');
            i += 2;
            continue;
        }
        if ch == 'r' && next == Some('r') {
            out.push('r');
            i += 2;
            continue;
        }
        if ch == 'q' && next == Some('u') {
            out.push('k');
            i += 2;
            continue;
        }
        if ch == 'g'
            && next == Some('u')
            && let Some(after) = chars.get(i + 2).copied()
            && matches!(after, 'e' | 'é' | 'i' | 'í')
        {
            out.push('ɡ');
            i += 2;
            continue;
        }

        match ch {
            'a' | 'á' => out.push('a'),
            'e' | 'é' => out.push('e'),
            'i' | 'í' => out.push('i'),
            'o' | 'ó' => out.push('o'),
            'u' | 'ú' | 'ü' => out.push('u'),
            'b' | 'v' => out.push('b'),
            'c' => {
                if matches!(next, Some('e' | 'é' | 'i' | 'í')) {
                    out.push('s');
                } else {
                    out.push('k');
                }
            }
            'd' => out.push('d'),
            'f' => out.push('f'),
            'g' => {
                if matches!(next, Some('e' | 'é' | 'i' | 'í')) {
                    out.push('x');
                } else {
                    out.push('ɡ');
                }
            }
            'h' => {} // silent in Spanish
            'j' => out.push('x'),
            'k' => out.push('k'),
            'l' => out.push('l'),
            'm' => out.push('m'),
            'n' => out.push('n'),
            'ñ' => out.push('ɲ'),
            'p' => out.push('p'),
            'q' => out.push('k'),
            'r' => {
                let word_start = prev.is_none() || prev.is_some_and(|p| !p.is_alphabetic());
                if word_start || matches!(prev, Some('l' | 'n' | 's')) {
                    out.push('r');
                } else {
                    out.push('ɾ');
                }
            }
            's' | 'z' => out.push('s'),
            't' => out.push('t'),
            'w' => out.push('w'),
            'x' => out.push_str("ks"),
            'y' => {
                let next_is_alpha = next.is_some_and(|c| c.is_alphabetic());
                if next_is_alpha {
                    out.push('ʝ');
                } else {
                    out.push('i');
                }
            }
            '0' => out.push_str("seɾo "),
            '1' => out.push_str("uno "),
            '2' => out.push_str("dos "),
            '3' => out.push_str("tɾes "),
            '4' => out.push_str("kwatɾo "),
            '5' => out.push_str("sinko "),
            '6' => out.push_str("seis "),
            '7' => out.push_str("sjete "),
            '8' => out.push_str("otʃo "),
            '9' => out.push_str("nwebe "),
            _ => {}
        }
        i += 1;
    }

    out.trim().to_string()
}

/// Lightweight English fallback when `espeak-ng` is not installed.
fn phonemize_english_fallback(text: &str) -> String {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut out = String::with_capacity(chars.len() * 2);
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if ch.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            i += 1;
            continue;
        }
        if matches!(ch, '.' | ',' | ';' | ':' | '!' | '?') {
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == 's' && next == Some('h') {
            out.push('ʃ');
            i += 2;
            continue;
        }
        if ch == 'c' && next == Some('h') {
            out.push_str("tʃ");
            i += 2;
            continue;
        }
        if ch == 't' && next == Some('h') {
            out.push('θ');
            i += 2;
            continue;
        }
        if ch == 'n' && next == Some('g') {
            out.push('ŋ');
            i += 2;
            continue;
        }
        if ch == 'p' && next == Some('h') {
            out.push('f');
            i += 2;
            continue;
        }
        match ch {
            'a' => out.push('æ'),
            'e' => out.push('ɛ'),
            'i' => out.push('ɪ'),
            'o' => out.push('ɒ'),
            'u' => out.push('ʌ'),
            'c' => out.push('k'),
            'g' => out.push('ɡ'),
            'j' => out.push_str("dʒ"),
            'q' => out.push('k'),
            'r' => out.push('ɹ'),
            'x' => out.push_str("ks"),
            'y' => out.push('j'),
            'b' | 'd' | 'f' | 'h' | 'k' | 'l' | 'm' | 'n' | 'p' | 's' | 't' | 'v' | 'w' | 'z' => {
                out.push(ch)
            }
            _ => {}
        }
        i += 1;
    }
    out.trim().to_string()
}

/// Maps IPA phonemes into Kokoro `input_ids` (`[0, ...tokens..., 0]`).
pub fn phonemes_to_kokoro_ids(phonemes: &str, vocab: &HashMap<char, i64>) -> Vec<i64> {
    let mut ids = Vec::with_capacity(phonemes.chars().count().min(KOKORO_MAX_TOKENS) + 2);
    ids.push(0);
    for ch in phonemes.chars() {
        if ids.len() > KOKORO_MAX_TOKENS {
            break;
        }
        if let Some(&id) = vocab.get(&ch) {
            ids.push(id);
        }
    }
    ids.push(0);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_phonemizer_handles_digraphs_and_silent_h() {
        let ipa = phonemize_spanish("¡Hola, chico! Año y perro.");
        assert!(ipa.contains("ola"), "silent h dropped: {ipa}");
        assert!(ipa.contains("tʃiko"), "ch -> tʃ: {ipa}");
        assert!(ipa.contains("aɲo"), "ñ -> ɲ: {ipa}");
        assert!(ipa.contains("pero"), "rr -> r: {ipa}");
    }

    #[test]
    fn phonemes_map_to_padded_kokoro_ids() {
        let mut vocab = HashMap::new();
        vocab.insert('o', 10);
        vocab.insert('l', 11);
        vocab.insert('a', 12);
        let ids = phonemes_to_kokoro_ids("ola!", &vocab);
        assert_eq!(ids, vec![0, 10, 11, 12, 0]);
    }
}
