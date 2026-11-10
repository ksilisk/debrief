//! Lines Whisper is known to invent: subtitle credits and sound tags it learned
//! from video subtitles, and one line over and over.

use debrief_core::transcript::Line;

const PHRASES: [&str; 13] = [
    "продолжение следует",
    "субтитры сделал",
    "субтитры создавал",
    "редактор субтитров",
    "спасибо за просмотр",
    "подписывайтесь на канал",
    "ставьте лайк",
    "добро пожаловать в наш канал",
    "dimatorzok",
    "amara org",
    "thanks for watching",
    "subtitles by",
    "please subscribe",
];
/// A real sentence can quote a phrase; an invented line is little else.
const MAX_WORDS: usize = 8;
/// This many identical lines in a row is a loop; the first one stays.
const LOOP_LINES: usize = 3;

pub fn without_invented(lines: Vec<Line>) -> Vec<Line> {
    let mut kept: Vec<Line> = Vec::with_capacity(lines.len());
    let mut run = 0;
    let mut previous = String::new();
    for line in lines {
        let text = normalize(&line.text);
        run = if !text.is_empty() && text == previous {
            run + 1
        } else {
            1
        };
        previous.clone_from(&text);
        if is_invented(&line.text, &text) || run > LOOP_LINES {
            continue;
        }
        if run == LOOP_LINES {
            // The copies kept so far go too, all but the first.
            kept.truncate(kept.len() + 2 - LOOP_LINES);
            continue;
        }
        kept.push(line);
    }
    kept
}

fn is_invented(raw: &str, text: &str) -> bool {
    let trimmed = raw.trim();
    let tag = [('*', '*'), ('[', ']'), ('(', ')')]
        .iter()
        .any(|&(open, close)| trimmed.starts_with(open) && trimmed.ends_with(close));
    text.is_empty()
        || tag
        || (text.split(' ').count() <= MAX_WORDS
            && PHRASES.iter().any(|phrase| text.contains(phrase)))
}

/// Lowercase letters and digits, single spaces between words.
fn normalize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ё' => 'е',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Vec<Line> {
        texts
            .iter()
            .enumerate()
            .map(|(i, text)| Line {
                start: i as f64,
                end: i as f64 + 1.0,
                text: (*text).into(),
            })
            .collect()
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn subtitle_credits_and_sound_tags_go() {
        let kept = without_invented(lines(&[
            "Продолжение следует...",
            "Субтитры сделал DimaTorzok",
            "*звук текста*",
            "[музыка]",
            "Ну, по идее, просто как поле в ответе.",
            "...",
        ]));
        assert_eq!(texts(&kept), ["Ну, по идее, просто как поле в ответе."]);
    }

    #[test]
    fn a_sentence_that_quotes_a_phrase_stays() {
        let sentence = "Давай договоримся, что продолжение следует на следующей неделе, когда Саша вернется из отпуска.";
        assert_eq!(texts(&without_invented(lines(&[sentence]))), [sentence]);
    }

    #[test]
    fn a_loop_keeps_its_first_line() {
        let kept = without_invented(lines(&[
            "Ну, да, да.",
            "Ну, да, да.",
            "Ну, да, да.",
            "Ну, да, да.",
            "Вот.",
            "Да.",
            "Да.",
        ]));
        assert_eq!(texts(&kept), ["Ну, да, да.", "Вот.", "Да.", "Да."]);
        assert_eq!(kept[0].start, 0.0);
    }
}
