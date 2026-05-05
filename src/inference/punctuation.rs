//! Lightweight rule-based Japanese punctuation restoration.
//!
//! ReazonSpeech-k2-v2 is trained on unpunctuated or inconsistently punctuated
//! data, so the model rarely emits `。` or `？`.  This module restores them
//! using simple grammatical heuristics so that the final transcript is
//! readable and scores well on CER benchmarks.

/// Append the most likely sentence-ending punctuation to a Japanese string.
///
/// Rules (applied in order):
/// 1. Question → `？` if the text ends with a question pattern (`か`, `ですか`, `ますか`, etc.)
/// 2. Exclamation → `！` if it ends with exclamatory particles (`ね！`, `よ！`, `わ！` are rare in ASR)
/// 3. Statement → `。` otherwise
///
/// The function is intentionally conservative — it only appends punctuation
/// when the text does not already end with a known sentence terminator.
pub fn auto_punctuate(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }

    // Already punctuated — don't touch.
    if ends_with_punct(text) {
        return text.to_string();
    }

    if is_question(text) {
        format!("{}？", text)
    } else {
        format!("{}。", text)
    }
}

fn ends_with_punct(s: &str) -> bool {
    matches!(
        s.chars().last(),
        Some('。') | Some('？') | Some('！') | Some('.') | Some('?') | Some('!')
    )
}

fn is_question(s: &str) -> bool {
    let q_patterns = [
        "ですか",
        "ますか",
        "ましたか",
        "ませんか",
        "でしょうか",
        "ましょうか",
        "か？",
        "か。",
    ];
    for pat in &q_patterns {
        if s.ends_with(pat) {
            return true;
        }
    }
    // Ends with standalone か (common in short questions like 誰か, 何か)
    // but avoid false positives on words ending in か (e.g. 作家, 科学).
    // Heuristic: if the last char is か and the text is short (<10 chars) it's likely a question.
    if s.ends_with('か') && s.chars().count() < 10 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_question_long() {
        assert_eq!(
            auto_punctuate("昨日はよく眠れましたか"),
            "昨日はよく眠れましたか？"
        );
    }

    #[test]
    fn test_question_masenka() {
        assert_eq!(auto_punctuate("座りませんか"), "座りませんか？");
    }

    #[test]
    fn test_question_short() {
        assert_eq!(auto_punctuate("誰か"), "誰か？");
    }

    #[test]
    fn test_question_desuka() {
        assert_eq!(
            auto_punctuate("集合場所はどこですか"),
            "集合場所はどこですか？"
        );
    }

    #[test]
    fn test_statement() {
        assert_eq!(auto_punctuate("動かないでください"), "動かないでください。");
    }

    #[test]
    fn test_already_punctuated() {
        assert_eq!(auto_punctuate("こんにちは。"), "こんにちは。");
        assert_eq!(auto_punctuate("何ですか？"), "何ですか？");
    }

    #[test]
    fn test_empty() {
        assert_eq!(auto_punctuate(""), "");
    }
}
