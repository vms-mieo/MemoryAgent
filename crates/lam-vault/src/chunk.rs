pub const MIN_CHARS: usize = 400;
pub const MAX_CHARS: usize = 1000;
pub const OVERLAP_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub ordinal: usize,
    pub content: String,
    pub line_start: u32,
    pub line_end: u32,
    pub hash: String,
}

pub fn chunk_markdown(text: &str) -> Vec<Chunk> {
    chunk_lines(text, true)
}

pub fn chunk_text(text: &str) -> Vec<Chunk> {
    chunk_lines(text, false)
}

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return false;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && trimmed[hashes..].starts_with([' ', '\t'])
}

fn overlap_tail(previous: &str) -> String {
    if previous.chars().count() <= OVERLAP_CHARS {
        return String::new();
    }
    let tail: String = previous
        .chars()
        .rev()
        .take(OVERLAP_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    match tail.find(char::is_whitespace) {
        Some(position) => tail[position..].trim_start().to_string(),
        None => String::new(),
    }
}

fn chunk_lines(text: &str, heading_aware: bool) -> Vec<Chunk> {
    let normalized = text.replace("\r\n", "\n");
    if normalized.trim().is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = normalized.lines().collect();
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for (index, line) in lines.iter().enumerate() {
        if heading_aware && is_heading(line) && index > start {
            segments.push((start, index - 1));
            start = index;
        }
    }
    segments.push((start, lines.len() - 1));
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_chars = 0usize;
    let mut ordinal = 0usize;
    let flush = |current: &mut Vec<usize>, chunks: &mut Vec<Chunk>, ordinal: &mut usize| {
        if current.is_empty() {
            return;
        }
        let first = current[0];
        let mut last = *current.last().expect("non-empty");
        while last > first && lines[last].trim().is_empty() {
            last -= 1;
        }
        let body: Vec<&str> = current
            .iter()
            .filter(|index| **index >= first && **index <= last)
            .map(|index| lines[*index])
            .collect();
        let mut content = body.join("\n");
        if let Some(previous) = chunks.last() {
            let tail = overlap_tail(&previous.content);
            if !tail.is_empty() {
                content = format!("{tail}\n{content}");
            }
        }
        chunks.push(Chunk {
            ordinal: *ordinal,
            content: content.clone(),
            line_start: (first + 1) as u32,
            line_end: (last + 1) as u32,
            hash: crate::hash::blake3_hex(content.as_bytes()),
        });
        *ordinal += 1;
        current.clear();
    };
    for (index, (seg_start, seg_end)) in segments.iter().enumerate() {
        let seg_chars: usize = lines[*seg_start..=*seg_end]
            .iter()
            .map(|line| line.chars().count() + 1)
            .sum();
        if !current.is_empty()
            && (current_chars + seg_chars > MAX_CHARS
                || (current_chars >= MIN_CHARS && heading_aware))
        {
            flush(&mut current, &mut chunks, &mut ordinal);
            current_chars = 0;
        }
        if seg_chars > MAX_CHARS {
            let mut piece: Vec<usize> = Vec::new();
            let mut piece_chars = 0usize;
            for (line_number, line) in lines.iter().enumerate().take(*seg_end + 1).skip(*seg_start)
            {
                piece.push(line_number);
                piece_chars += line.chars().count() + 1;
                let boundary =
                    line.trim().is_empty() || index + 1 < segments.len() || line_number == *seg_end;
                if piece_chars >= MAX_CHARS && boundary || piece_chars >= MAX_CHARS + 200 {
                    let mut standalone = piece.clone();
                    let mut fake_ordinal = ordinal;
                    flush(&mut standalone, &mut chunks, &mut fake_ordinal);
                    ordinal = fake_ordinal;
                    piece.clear();
                    piece_chars = 0;
                }
            }
            if !piece.is_empty() {
                current.extend(piece.iter());
                current_chars += piece
                    .iter()
                    .map(|line_number| lines[*line_number].chars().count() + 1)
                    .sum::<usize>();
            }
            continue;
        }
        for line_number in *seg_start..=*seg_end {
            current.push(line_number);
        }
        current_chars += seg_chars;
    }
    flush(&mut current, &mut chunks, &mut ordinal);
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn long_doc() -> String {
        let mut out = String::from("# Guide\n\nIntro paragraph with enough words to matter.\n\n");
        for section in 0..12 {
            out.push_str(&format!("## Section {section}\n\n"));
            for _ in 0..8 {
                out.push_str("Lorem ipsum dolor sit amet, consectetur adipiscing elit sed do.\n");
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(chunk_markdown("   \n  ").is_empty());
        assert!(chunk_text("").is_empty());
    }

    #[test]
    fn small_doc_is_single_chunk_with_full_range() {
        let text = "# Title\n\nTwo short lines here.\n";
        let chunks = chunk_markdown(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].line_start, 1);
        assert!(chunks[0].content.contains("Title"));
        assert_eq!(chunks[0].hash.len(), 64);
    }

    #[test]
    fn chunks_cover_every_content_line() {
        let text = long_doc();
        let file_lines: Vec<&str> = text.lines().collect();
        let chunks = chunk_markdown(&text);
        assert!(chunks.len() > 1);
        let mut covered = HashSet::new();
        for chunk in &chunks {
            assert!(chunk.content.chars().count() <= MAX_CHARS + 220);
            for number in chunk.line_start..=chunk.line_end {
                covered.insert(number);
            }
        }
        for (index, line) in file_lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            assert!(
                covered.contains(&((index + 1) as u32)),
                "line {index} uncovered"
            );
        }
    }

    #[test]
    fn citations_point_at_included_lines() {
        let text = long_doc();
        let file_lines: Vec<&str> = text.lines().collect();
        for chunk in chunk_markdown(&text) {
            let cited: Vec<&str> =
                file_lines[(chunk.line_start as usize - 1)..(chunk.line_end as usize)].to_vec();
            let cited_text = cited.join("\n");
            assert!(
                chunk.content.ends_with(&cited_text),
                "chunk {} must end with its cited lines",
                chunk.ordinal
            );
        }
    }

    #[test]
    fn later_chunks_carry_overlap() {
        let chunks = chunk_markdown(&long_doc());
        assert!(chunks.len() > 1);
        let tail = overlap_tail(&chunks[0].content);
        assert!(!tail.is_empty());
        assert!(chunks[1].content.starts_with(&tail));
    }

    #[test]
    fn chunk_ids_are_stable() {
        let text = long_doc();
        let first = chunk_markdown(&text);
        let second = chunk_markdown(&text);
        assert_eq!(
            first.iter().map(|c| &c.hash).collect::<Vec<_>>(),
            second.iter().map(|c| &c.hash).collect::<Vec<_>>()
        );
    }
}
