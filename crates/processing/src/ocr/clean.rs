//! Vision-model OCR output cleaning: strip echo artifacts before the text
//! reaches a user (clipboard for screenshot OCR, imported documents for the
//! file OCR path).
//!
//! Some OCR-finetuned models (observed with glm-ocr across 2026-09-06/08)
//! misbehave in three ways, all handled here:
//!
//! 1. **Fenced echo + fence tail** — the extraction repeated inside a
//!    ```-fenced block, padded with dozens of bare fence lines.
//! 2. **Plain echo** — the extraction appended verbatim (or nearly: rewrapped,
//!    or with a word of per-copy OCR noise), once or as a degenerating loop
//!    that mutates and dissolves into token soup.
//! 3. **Degenerate echo loop** — many exact copies, then mutated copies, then
//!    garbage.
//!
//! The stages run in order: fence unwrap → line-verbatim echo collapse
//! (with confirmed-loop salvage) → word-stream collapse for rewrapped/noisy
//! two-copy echoes. Well-behaved extractions pass through untouched.

/// Clean a vision model's OCR output before it is used.
///
/// When the output contains any fenced block, prefer the text OUTSIDE the
/// fences (the plain extraction); when everything is fenced, take the inside
/// of the first block. Fence-free output passes through to the echo stages.
///
/// Trade-off: a screenshot OF markdown source (where fences are content)
/// loses its fenced sections. For a quick-capture tool, clean text is the
/// better default.
pub fn clean_ocr_text(raw: &str) -> String {
    let trimmed = raw.trim();
    let unfenced = if !trimmed.contains("```") {
        trimmed.to_string()
    } else {
        let mut outside: Vec<&str> = Vec::new();
        let mut inside_first: Option<Vec<&str>> = None;
        let mut in_fence = false;
        for line in trimmed.lines() {
            if line.trim_start().starts_with("```") {
                if !in_fence && inside_first.is_none() {
                    inside_first = Some(Vec::new());
                }
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                if let Some(lines) = inside_first.as_mut() {
                    lines.push(line);
                }
            } else {
                outside.push(line);
            }
        }
        let outside_text = outside.join("\n").trim().to_string();
        if !outside_text.is_empty() {
            outside_text
        } else if let Some(lines) = inside_first {
            let inside = lines.join("\n").trim().to_string();
            if !inside.is_empty() {
                inside
            } else {
                trimmed.to_string()
            }
        } else {
            trimmed.to_string()
        }
    };
    // The echo isn't always fenced — glm-ocr sometimes repeats the whole
    // selection as plain text (identical lines appended verbatim).
    let deduped = dedupe_exact_repeat(&unfenced);
    // Line-level matching misses echoes whose copies differ by line
    // wrapping or a word of OCR noise (observed 2026-09-08): a
    // word-stream comparison tolerates both.
    collapse_word_normalized_repeat(&deduped)
}

fn is_separator_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.starts_with("```") {
        return true;
    }
    // Markdown horizontal rules (`---`, `***`, `___`, spaced variants) —
    // vision models commonly divide an extraction from its echo with one.
    let unspaced: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    is_horizontal_rule(&unspaced)
}

/// A run of 3+ of a single markdown rule marker char (`-`/`*`/`_`).
fn is_horizontal_rule(s: &str) -> bool {
    s.len() >= 3
        && (s.chars().all(|c| c == '-')
            || s.chars().all(|c| c == '*')
            || s.chars().all(|c| c == '_'))
}

/// Words of a line for echo-residue comparison: lowercase, punctuation
/// stripped, order-insensitive.
fn word_set(line: &str) -> std::collections::HashSet<String> {
    line.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Does `line` look like a (mutated) copy of some unit line — at least 60%
/// of its words appear in one unit line? Used ONLY after an echo loop is
/// confirmed (≥3 exact consecutive copies), to tell a degenerated echo tail
/// (resembling) from genuinely fresh content (not resembling). A line with
/// no alphanumeric words at all (",,," / "or ...") counts as residue.
fn resembles_unit_line(line: &str, unit: &[&str]) -> bool {
    let words = word_set(line);
    if words.is_empty() {
        return true; // punctuation/token soup — pure model breakdown
    }
    unit.iter().any(|u| {
        let unit_words = word_set(u);
        let hits = words.iter().filter(|w| unit_words.contains(*w)).count();
        hits * 10 >= words.len() * 6
    })
}

/// Collapse whole-text verbatim repetitions to a single copy: vision OCR
/// models (observed with glm-ocr) sometimes append one or more echoes of
/// the entire extraction, separated by a blank line, a bare fence, or a
/// markdown horizontal rule.
///
/// The unit is searched SHORTEST-first: any echo of k copies also matches
/// with a unit of k/2 copies, so a longest-first search would collapse a
/// 10-copy echo to 5 copies instead of 1.
///
/// Special case first — ALL content lines identical: repeated FORM ROWS are
/// content, so the text collapses only for a plain two-copy echo (any row
/// count); three or more identical rows always stay. Without this, an even
/// count of identical rows (4, 6, …) would collapse to half via the
/// multi-line-unit view, contradicting the three-rows-stay rule.
///
/// Otherwise, two collapse modes:
///
/// 1. Exact-whole-text (conservative, unchanged in spirit since the first
///    echo fix): the copies (with separator lines between/after) consume
///    the text exactly, compared line-by-line modulo whitespace.
///    A single-LINE unit may collapse only with exactly two copies (three
///    identical rows in a form are content, not an echo); a multi-line
///    unit may collapse for any copy count.
///
/// 2. Confirmed echo loop with a degenerate tail (2026-09-08 user report):
///    ≥3 exact consecutive copies of a multi-line unit opening the text,
///    followed by a NON-empty remainder. Real documents never open with
///    the same multi-line stanza three times in a row — that is a model
///    echo loop, and in the observed failure the loop then DEGENERATED
///    (later copies mutate, shed lines, and finally dissolve into token
///    soup like "such, such,," / ",, or"), which broke the exact-whole-
///    text requirement and shipped the whole loop to the clipboard. The
///    salvage keeps ONE unit; the remainder is kept only when its first
///    substantive line does NOT resemble any unit line (genuinely fresh
///    content after the loop) and dropped when it resembles (a mutated
///    echo) or carries no words at all (breakdown soup).
fn dedupe_exact_repeat(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    // All content lines identical: a plain two-copy echo (of any row
    // count) collapses; three or more identical rows are form content.
    let content: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !is_separator_line(l))
        .collect();
    if content.len() >= 2 && content.windows(2).all(|w| w[0].trim() == w[1].trim()) {
        if content.len() == 2 {
            return content[0].trim().to_string();
        }
        return text.to_string();
    }

    // Shortest unit first: prefer collapsing to the minimal repeating unit.
    for unit_len in 1..=lines.len() / 2 {
        let unit = &lines[..unit_len];
        // The repeated unit must carry content — never collapse to blanks.
        if unit.iter().all(|l| is_separator_line(l)) {
            continue;
        }
        let mut i = unit_len;
        let mut copies = 1usize;
        loop {
            // Separators may sit between copies and trail the last one
            // (bare-fence tails).
            while i < lines.len() && is_separator_line(lines[i]) {
                i += 1;
            }
            if lines.len() - i >= unit_len
                && unit
                    .iter()
                    .zip(lines[i..].iter())
                    .all(|(a, b)| a.trim() == b.trim())
            {
                i += unit_len;
                copies += 1;
            } else {
                break;
            }
        }
        // Edge-trim separators from the unit for the collapsed output.
        let mut start = 0;
        let mut end = unit.len();
        while start < end && is_separator_line(unit[start]) {
            start += 1;
        }
        while end > start && is_separator_line(unit[end - 1]) {
            end -= 1;
        }
        let joined = unit[start..end].join("\n");

        // Mode 1: the exact copies consume the whole text.
        if i == lines.len()
            && copies >= 2
            && (unit_len >= 2 || copies == 2)
            && !joined.trim().is_empty()
        {
            return joined;
        }

        // Mode 2: confirmed echo loop with a remainder. The separator skip
        // above may have walked past trailing blanks/fences up to the first
        // substantive remainder line already; back i up to the end of the
        // last exact copy (no further — a unit can contain internal
        // separator lines) so separators/fresh content are not lost.
        if copies >= 3 && unit_len >= 2 && i < lines.len() {
            let floor = copies * unit_len;
            let mut remainder_start = i;
            while remainder_start > floor && is_separator_line(lines[remainder_start - 1]) {
                remainder_start -= 1;
            }
            let remainder = &lines[remainder_start..];
            let first_substantive = remainder.iter().find(|l| !is_separator_line(l));
            match first_substantive {
                Some(line) if !resembles_unit_line(line, unit) => {
                    // Fresh content after the loop — keep it.
                    return format!("{joined}\n{}", remainder.join("\n"));
                }
                _ => {
                    // Degenerated echo / breakdown soup — salvage one unit.
                    return joined;
                }
            }
        }
    }
    text.to_string()
}

/// Whitespace-noise-tolerant two-copy echo collapse (2026-09-08, second
/// user report). `dedupe_exact_repeat` compares line-by-line verbatim, so
/// an echo whose second copy REWRAPS the text at different points — or
/// carries one word of OCR noise ("Subject-Clien" vs "Subject-Client") —
/// defeats it and both copies reach the user. This stage compares
/// the text as a flattened WORD stream instead: wrapping is invisible and
/// a bounded word-edit tolerance absorbs per-copy noise.
///
/// A collapse requires the whole text to be exactly one copy plus one
/// more copy (complete, or truncated by a max_tokens cut — the tail must
/// still be at least half the head), with ≤5% differing words, and the
/// split point must land on a line boundary so the output keeps the
/// FIRST copy's own line breaks.
///
/// Conservatism: when every content line is identical, the shape is the
/// line-level rule's business (three identical form rows are content) —
/// refuse, so this stage can never override that decision. Giant inputs
/// (a degenerate loop the salvage should have handled) are skipped for
/// time; the edit-distance pass is quadratic.
fn collapse_word_normalized_repeat(text: &str) -> String {
    const SKIP_ABOVE_WORDS: usize = 2000;
    const MIN_WORDS: usize = 12;

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return text.to_string();
    }
    // Word stream over content lines, remembering each content line's
    // cumulative word count so the split can align with a line end.
    let mut words: Vec<&str> = Vec::new();
    let mut line_end_words: Vec<usize> = Vec::new();
    let mut line_index: Vec<usize> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if is_separator_line(line) {
            continue;
        }
        words.extend(line.split_whitespace());
        line_end_words.push(words.len());
        line_index.push(idx);
    }
    let n = words.len();
    if !(MIN_WORDS..=SKIP_ABOVE_WORDS).contains(&n) {
        return text.to_string();
    }
    // All-identical content lines: the line-level rule owns this shape
    // (and may have deliberately kept it as content).
    let first = lines.iter().find(|l| !is_separator_line(l));
    if let Some(first) = first
        && lines
            .iter()
            .filter(|l| !is_separator_line(l))
            .all(|l| l.trim() == first.trim())
    {
        return text.to_string();
    }

    // Candidate split points: the exact half (complete second copy), then
    // truncated second copies, longest tail first. The tail must be at
    // least half the head (a max_tokens cut, not a repeated opening
    // phrase).
    let mut candidates: Vec<usize> = Vec::new();
    if n.is_multiple_of(2) {
        candidates.push(n / 2);
    }
    let h_min = n / 2 + 1;
    let h_max = (2 * n / 3).min(n.saturating_sub(4));
    let mut h = h_max;
    while h >= h_min {
        candidates.push(h);
        h -= 1;
    }

    for &h in &candidates {
        // Compare the tail against the head's prefix OF THE TAIL'S LENGTH:
        // for a truncated second copy the length difference is the
        // legitimate cut, not noise — only word-level differences count.
        let tail_len = n - h;
        let head_prefix = words[..tail_len].join(" ");
        let tail = words[h..].join(" ");
        let (dist, _) = crate::edit_distance::word_edit_distance(&head_prefix, &tail);
        // ≤5% differing words (at least one word of slack).
        if dist > (tail_len / 20).max(1) {
            continue;
        }
        // The split must land at the end of one of the first copy's lines.
        let Some(pos) = line_end_words.iter().position(|&e| e == h) else {
            continue;
        };
        let last_idx = line_index[pos];
        let mut out: Vec<&str> = lines[..=last_idx].to_vec();
        while out.last().is_some_and(|l| is_separator_line(l)) {
            out.pop();
        }
        let joined = out.join("\n");
        if !joined.trim().is_empty() {
            return joined;
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_fence_free_output_through() {
        assert_eq!(clean_ocr_text("  HbA1c 7.2 %  "), "HbA1c 7.2 %");
        assert_eq!(clean_ocr_text("line one\nline two\n"), "line one\nline two");
        assert_eq!(clean_ocr_text("   "), "");
    }

    #[test]
    fn strips_glm_ocr_echo_and_fence_tail() {
        // The exact shape glm-ocr produced in the live dry-run (2026-09-06):
        // clean extraction, then a fenced duplicate, then dozens of bare
        // fences (elided here to a few).
        let raw = "HbA1c 7.2 %\n\nNext review: 3 months\n```markdown\n\nHbA1c 7.2 %\n\nNext review: 3 months\n```\n```\n```\n```\n       \n```";
        assert_eq!(clean_ocr_text(raw), "HbA1c 7.2 %\n\nNext review: 3 months");
    }

    #[test]
    fn unwraps_fully_fenced_output() {
        let raw = "```markdown\nHbA1c 7.2 %\n```\n```";
        assert_eq!(clean_ocr_text(raw), "HbA1c 7.2 %");
    }

    #[test]
    fn falls_back_to_raw_when_fences_hold_nothing() {
        // Degenerate: fences but no text anywhere usable.
        assert_eq!(clean_ocr_text("```\n\n```\n```"), "```\n\n```\n```".trim());
    }

    #[test]
    fn collapses_plain_verbatim_echo() {
        // The user's report (2026-09-06): glm-ocr repeated the whole
        // selection as plain text, no fences involved.
        let line = "AppError struct ( {kind, message} ),";
        assert_eq!(clean_ocr_text(&format!("{line}\n{line}")), line);
    }

    #[test]
    fn collapses_multi_line_echo_with_blank_separator() {
        let block = "HbA1c: 7.2 %\nBP: 128/76";
        assert_eq!(clean_ocr_text(&format!("{block}\n\n{block}\n")), block);
    }

    #[test]
    fn keeps_repeated_line_inside_larger_document() {
        // A repeated form label inside a longer (non-echo) extraction must
        // NOT collapse — the echo must consume the ENTIRE remainder.
        let doc = "Weight: 70 kg\nWeight: 70 kg\nHeight: 175 cm\nBP: 128/76";
        assert_eq!(clean_ocr_text(doc), doc);
    }

    #[test]
    fn keeps_three_identical_rows() {
        // Exact-two-copies rule: a 3x repeated row is content, not an echo.
        let rows = "N/A\nN/A\nN/A";
        assert_eq!(clean_ocr_text(rows), rows);
    }

    /// Consistency pin (2026-09-08 pipeline review): identical rows are
    /// content at ANY count ≥ 3 — previously an even count (4, 6, …)
    /// collapsed to half via the multi-line-unit view, contradicting the
    /// three-rows-stay rule.
    #[test]
    fn keeps_identical_rows_at_any_count_above_two() {
        for count in [4usize, 5, 6] {
            let rows: String = (0..count).map(|_| "Not applicable here sir.\n").collect();
            let rows = rows.trim_end();
            assert_eq!(clean_ocr_text(rows), rows, "{count} rows must stay");
        }
    }

    /// The plain two-copy echo of identical-row content still collapses
    /// (one original + one echo), for single- and multi-word rows.
    #[test]
    fn collapses_two_identical_rows_as_a_plain_echo() {
        assert_eq!(clean_ocr_text("N/A\nN/A"), "N/A");
        let row = "Not applicable here sir.";
        assert_eq!(clean_ocr_text(&format!("{row}\n{row}")), row);
    }

    #[test]
    fn dedupe_runs_after_fence_unwrap() {
        // Fenced echo where the OUTSIDE text itself is echoed: fence stage
        // yields the doubled plain text, dedupe then collapses it.
        let line = "Med list: aspirin";
        let raw = format!("{line}\n{line}\n```markdown\n{line}\n```\n```");
        assert_eq!(clean_ocr_text(&raw), line);
    }

    /// A pure multi-copy echo collapses to ONE copy, not half — the unit
    /// search must run shortest-first (any k-copy echo also matches with a
    /// k/2-copy unit, so longest-first kept 2 of 4 copies).
    #[test]
    fn collapses_quadruple_echo_to_one_copy() {
        let block = "HbA1c: 7.2 %\nBP: 128/76";
        let raw = format!("{block}\n\n{block}\n\n{block}\n\n{block}\n");
        assert_eq!(clean_ocr_text(&raw), block);
    }

    /// THE 2026-09-08 user report shape: ~100 exact copies, then copies
    /// that MUTATE and shed lines, then pure token soup — the degeneration
    /// broke the exact-whole-text requirement and the whole loop reached
    /// the clipboard. ≥3 exact consecutive copies confirm an echo loop;
    /// the salvage keeps one unit and drops the resembling/soup tail.
    #[test]
    fn salvages_degenerate_echo_loop() {
        let report = "EXAM TYPE:\nAP/PA weight-bearing, lateral and skyline view left knee x-ray\nCOMPARISON:\nNo previous for comparison.\nFINDINGS:\nMild joint space narrowing medial compartment.";
        let mut raw = String::new();
        for _ in 0..6 {
            raw.push_str(report);
            raw.push('\n');
        }
        // Degenerated copies: mutated exam line, shed lines, then soup.
        raw.push_str("EXAM TYPE:\nAP/PA weight-bearing, lateral view left knee x-ray\nCOMPARISON:\nNo previous for comparison.\n");
        raw.push_str("COMPARISON:\nNo previous for comparison.\nFINDINGS:\nMild joint space narrowing medics,待\n");
        raw.push_str(",,,, or\nsuch, such, such\n...\n");

        assert_eq!(clean_ocr_text(&raw), report);
    }

    /// Fresh content after a confirmed echo loop SURVIVES the salvage: the
    /// remainder's first substantive line shares no 60%-word overlap with
    /// any unit line, so it is content, not residue.
    #[test]
    fn keeps_fresh_content_after_confirmed_echo_loop() {
        let stanza = "EXAM TYPE:\nLeft knee x-ray";
        let tail = "IMPRESSION:\nEarly tricompartmental osteoarthritis.";
        let raw = format!("{stanza}\n\n{stanza}\n\n{stanza}\n\n{tail}");
        // The separating blank line survives with the fresh content.
        assert_eq!(clean_ocr_text(&raw), format!("{stanza}\n\n{tail}"));
    }

    /// Two exact copies + other content is BELOW the echo-loop threshold —
    /// the conservative exact-whole-text rule alone applies, and a partial
    /// echo passes through unchanged (as it always has).
    #[test]
    fn below_three_copies_never_triggers_the_salvage() {
        let stanza = "EXAM TYPE:\nLeft knee x-ray";
        let tail = "IMPRESSION:\nSomething else entirely.";
        let raw = format!("{stanza}\n{stanza}\n{tail}");
        assert_eq!(clean_ocr_text(&raw), raw);
    }

    /// THE 2026-09-08 second user report shape: two copies of the same
    /// note where the second copy REWRAPS at different points — the
    /// line-verbatim rule never matches, the word-stream stage collapses
    /// to the first copy (keeping its own line breaks).
    #[test]
    fn collapses_rewrapped_two_copy_echo() {
        let copy1 = "Phone Call Appointment Note: Victoria understands and accepts\nthe limitations and expectations of Virtual Care.\nSubject-Client complaint: Ongoing loose, watery stools.";
        let copy2 = "Phone Call Appointment Note: Victoria understands and accepts the\nlimitations and expectations of Virtual Care. Subject-Client\ncomplaint: Ongoing loose, watery stools.";
        let raw = format!("{copy1}\n{copy2}");
        assert_eq!(clean_ocr_text(&raw), copy1);
    }

    /// Per-copy OCR noise ("Subject-Clien" vs "Subject-Client") — one word
    /// of edit distance is inside the 5% tolerance.
    #[test]
    fn collapses_two_copy_echo_with_one_noisy_word() {
        let copy1 = "Phone Call Appointment Note: Victoria understands and accepts the limitations and expectations of Virtual Care. Subject-Client complaint: Ongoing loose watery stools.";
        let copy2 = "Phone Call Appointment Note: Victoria understands and accepts the limitations and expectations of Virtual Care. Subject-Clien complaint: Ongoing loose watery stools.";
        let raw = format!("{copy1}\n{copy2}");
        assert_eq!(clean_ocr_text(&raw), copy1);
    }

    /// A max_tokens cut mid-second-copy: the tail is still ≥ half the head
    /// and word-matches the head's opening — keep the complete first copy.
    #[test]
    fn collapses_truncated_second_copy() {
        let words: Vec<String> = (0..30).map(|i| format!("word{i}")).collect();
        let head = words.join(" ");
        let tail = words[..20].join(" ");
        let raw = format!("{head}\n{tail}");
        assert_eq!(clean_ocr_text(&raw), head);
    }

    /// All-identical content lines are the LINE rule's shape (repeated
    /// form rows are content) — the WORD stage's guard must refuse so it
    /// can never override that decision.
    #[test]
    fn word_stage_never_overrides_identical_row_content() {
        let rows = "Not applicable here sir.\nNot applicable here sir.\nNot applicable here sir.";
        assert_eq!(collapse_word_normalized_repeat(rows), rows);
    }

    /// Two genuinely DIFFERENT paragraphs are not an echo — the 5% word
    /// tolerance must not bridge real content differences.
    #[test]
    fn keeps_two_different_paragraphs() {
        let a = "Phone Call Appointment Note: Victoria understands and accepts the limitations and expectations of Virtual Care entirely.";
        let b = "Plan: oral rehydration, return if symptoms persist beyond forty-eight hours or bloody diarrhea develops.";
        let raw = format!("{a}\n{b}");
        assert_eq!(clean_ocr_text(&raw), raw);
    }

    #[test]
    fn collapses_echo_separated_by_horizontal_rule() {
        // A markdown-flavored model divides the extraction from its echo
        // with `---` (also `***` / `___` / spaced variants).
        let block = "HbA1c: 7.2 %\nBP: 128/76";
        assert_eq!(clean_ocr_text(&format!("{block}\n---\n{block}")), block);
        assert_eq!(clean_ocr_text(&format!("{block}\n***\n{block}")), block);
        assert_eq!(clean_ocr_text(&format!("{block}\n_ _ _\n{block}")), block);
    }

    #[test]
    fn collapses_plain_echo_with_bare_fence_tail() {
        // Unfenced echo followed by stray fences (the glm-ocr tail without
        // the fenced duplicate).
        let line = "Next review: 3 months";
        assert_eq!(clean_ocr_text(&format!("{line}\n{line}\n```\n```")), line);
    }

    #[test]
    fn collapses_triple_multi_line_echo() {
        // A multi-line stanza echoed twice more (three copies) is an echo,
        // not content — the whole capture cannot be one stanza thrice.
        let block = "HbA1c: 7.2 %\nBP: 128/76";
        assert_eq!(
            clean_ocr_text(&format!("{block}\n\n{block}\n\n{block}\n")),
            block
        );
    }

    #[test]
    fn keeps_partial_repeat_inside_larger_document() {
        // A doubled final line under DIFFERENT preceding content is not a
        // whole-text echo — the echo must consume the entire text.
        let doc = "Weight: 70 kg\nHeight: 175 cm\nN/A\nN/A";
        assert_eq!(clean_ocr_text(doc), doc);
    }

    #[test]
    fn keeps_document_with_rule_between_distinct_blocks() {
        // Two different stanzas around a rule, and a rule inside content:
        // no verbatim copy → nothing collapses.
        let doc = "Page one text\n---\nPage two text";
        assert_eq!(clean_ocr_text(doc), doc);
        let doc2 = "----\nsignature line above\n----";
        assert_eq!(clean_ocr_text(doc2), doc2.trim());
    }
}
