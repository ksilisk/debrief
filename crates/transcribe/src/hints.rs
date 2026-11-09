//! Names and terms: spelling hints Whisper reads before each 30-second window,
//! and an exact count of their tokens that doesn't need the model loaded.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

/// How many tokens of hints whisper.cpp keeps: half of its 448-token text
/// context, less one for the marker in front. Longer hints lose their start.
pub const MAX_TOKENS: usize = 223;

/// "ggml", the first four bytes of a whisper.cpp model file.
const MAGIC: u32 = 0x6767_6d6c;
const HPARAMS: usize = 11;
/// No real token is this long; a bigger number means the file is damaged.
const MAX_TOKEN_BYTES: usize = 1024;

/// The text Whisper gets: the hints from each line, joined with commas.
pub fn prompt(hints: &str) -> String {
    let parts: Vec<&str> = hints
        .lines()
        .map(|line| line.trim().trim_end_matches(',').trim_end())
        .filter(|line| !line.is_empty())
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    // Whisper's own prompts start with a space, like text in the middle of a sentence.
    format!(" {}", parts.join(", "))
}

#[derive(Debug, thiserror::Error)]
#[error("could not read the vocabulary in {}: {source}", path.display())]
pub struct VocabError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// whisper.cpp's vocabulary, read from the header of the model file.
pub struct Vocab {
    tokens: HashMap<Vec<u8>, u32>,
}

impl Vocab {
    pub fn load(path: &Path) -> Result<Self, VocabError> {
        let error = |source| VocabError {
            path: path.to_path_buf(),
            source,
        };
        let file = File::open(path).map_err(error)?;
        Self::read(&mut BufReader::new(file)).map_err(error)
    }

    /// The layout `whisper_model_load` reads: magic, hyperparameters, mel
    /// filters, then each token as a length and its bytes.
    fn read(reader: &mut impl Read) -> io::Result<Self> {
        if read_u32(reader)? != MAGIC {
            return Err(invalid("this is not a whisper.cpp model"));
        }
        for _ in 0..HPARAMS {
            read_u32(reader)?;
        }
        let filter_bytes = u64::from(read_u32(reader)?) * u64::from(read_u32(reader)?) * 4;
        if io::copy(&mut reader.take(filter_bytes), &mut io::sink())? != filter_bytes {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let count = read_u32(reader)?;
        let mut tokens = HashMap::with_capacity(count as usize);
        for id in 0..count {
            let len = read_u32(reader)? as usize;
            if len > MAX_TOKEN_BYTES {
                return Err(invalid("a token is too long"));
            }
            let mut bytes = vec![0; len];
            reader.read_exact(&mut bytes)?;
            // A later token with the same bytes wins, as in whisper.cpp.
            tokens.insert(bytes, id);
        }
        Ok(Self { tokens })
    }

    /// Splits like `tokenize` in whisper.cpp: words first, then the longest
    /// known piece of each word; bytes no token covers are dropped.
    pub fn count(&self, text: &str) -> usize {
        words(text.as_bytes())
            .map(|word| self.count_word(word))
            .sum()
    }

    fn count_word(&self, word: &[u8]) -> usize {
        let mut count = 0;
        let mut start = 0;
        while start < word.len() {
            let longest = (start + 1..=word.len())
                .rev()
                .find(|&end| self.tokens.contains_key(&word[start..end]));
            match longest {
                Some(end) => {
                    count += 1;
                    start = end;
                }
                None => start += 1,
            }
        }
        count
    }
}

/// whisper.cpp's pattern
/// `'s|'t|'re|'ve|'m|'ll|'d| ?[[:alpha:]]+| ?[[:digit:]]+| ?[^\s[:alpha:][:digit:]]+|\s+(?!\S)|\s+`.
/// Its `std::regex` uses the C locale, so the classes are ASCII only and every
/// other byte, Cyrillic included, falls into the third group.
fn words(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let (word, tail) = rest.split_at(word_len(rest));
        rest = tail;
        Some(word)
    })
}

fn word_len(text: &[u8]) -> usize {
    const CONTRACTIONS: [&[u8]; 7] = [b"'s", b"'t", b"'re", b"'ve", b"'m", b"'ll", b"'d"];
    if let Some(contraction) = CONTRACTIONS.iter().find(|c| text.starts_with(c)) {
        return contraction.len();
    }
    let space = usize::from(text[0] == b' ');
    for class in [is_alpha, is_digit, is_other] {
        let run = text[space..]
            .iter()
            .take_while(|&&byte| class(byte))
            .count();
        if run > 0 {
            return space + run;
        }
    }
    let spaces = text.iter().take_while(|&&byte| is_space(byte)).count();
    // `\s+(?!\S)` leaves the last space to the word after it.
    if spaces > 1 && spaces < text.len() {
        spaces - 1
    } else {
        spaces
    }
}

fn is_alpha(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn is_other(byte: u8) -> bool {
    !is_space(byte) && !is_alpha(byte) && !is_digit(byte)
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        words(text.as_bytes())
            .map(|word| std::str::from_utf8(word).expect("whole characters"))
            .collect()
    }

    fn model_file(tokens: &[&str]) -> Vec<u8> {
        let mut file = Vec::new();
        file.extend(MAGIC.to_le_bytes());
        file.extend([0u8; HPARAMS * 4]);
        file.extend(2u32.to_le_bytes());
        file.extend(3u32.to_le_bytes());
        file.extend([0u8; 2 * 3 * 4]);
        file.extend((tokens.len() as u32).to_le_bytes());
        for token in tokens {
            file.extend((token.len() as u32).to_le_bytes());
            file.extend(token.as_bytes());
        }
        file
    }

    #[test]
    fn the_prompt_joins_lines_with_commas() {
        assert_eq!(
            prompt("Grafana,\n  Kubernetes\n\nTelemost, code freeze\n"),
            " Grafana, Kubernetes, Telemost, code freeze"
        );
        assert_eq!(prompt(" \n "), "");
    }

    #[test]
    fn words_split_like_whisper_cpp() {
        assert_eq!(
            split(" Grafana, Kubernetes"),
            [" Grafana", ",", " Kubernetes"]
        );
        assert_eq!(split("don't"), ["don", "'t"]);
        assert_eq!(split("v2.0 build"), ["v", "2", ".", "0", " build"]);
        assert_eq!(split("a   b  "), ["a", "  ", " b", "  "]);
        // Cyrillic letters are not [[:alpha:]] there, so they join the punctuation after them.
        assert_eq!(split(" Алексей, Jira"), [" Алексей,", " Jira"]);
    }

    #[test]
    fn counts_the_longest_known_pieces() {
        let file = model_file(&["G", "Gra", "fana", " Gra", ",", "a", "n"]);
        let vocab = Vocab::read(&mut file.as_slice()).expect("vocab");
        // " Grafana" → " Gra" + "fana"; "," → ","; "x" is unknown and dropped.
        assert_eq!(vocab.count(" Grafana,x"), 3);
        assert_eq!(vocab.count(""), 0);
    }

    #[test]
    fn a_file_that_is_not_a_model_fails() {
        assert!(Vocab::read(&mut b"not a model at all".as_slice()).is_err());
        let file = model_file(&["a", "b"]);
        assert!(Vocab::read(&mut &file[..file.len() - 1]).is_err());
    }
}
