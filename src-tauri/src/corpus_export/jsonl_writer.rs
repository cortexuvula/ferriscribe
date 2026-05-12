//! Writes the OpenAI chat-completion JSONL format.
//!
//! Each line is one JSON object:
//! {"messages":[
//!   {"role":"system","content":"<prompt template name>"},
//!   {"role":"user","content":"<redacted transcript>"},
//!   {"role":"assistant","content":"<redacted final SOAP>"}
//! ]}

use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct Record<'a> {
    messages: Vec<Message<'a>>,
}

pub struct TrainingRecord {
    pub system: String,
    pub user: String,
    pub assistant: String,
}

pub fn write_jsonl<W: Write>(
    writer: &mut W,
    records: impl IntoIterator<Item = TrainingRecord>,
) -> std::io::Result<usize> {
    let mut count = 0usize;
    for r in records {
        let record = Record {
            messages: vec![
                Message { role: "system", content: &r.system },
                Message { role: "user", content: &r.user },
                Message { role: "assistant", content: &r.assistant },
            ],
        };
        let line = serde_json::to_string(&record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_record_per_line() {
        let mut buf: Vec<u8> = Vec::new();
        let records = vec![
            TrainingRecord {
                system: "soap".to_string(),
                user: "transcript A".to_string(),
                assistant: "note A".to_string(),
            },
            TrainingRecord {
                system: "soap".to_string(),
                user: "transcript B".to_string(),
                assistant: "note B".to_string(),
            },
        ];
        let n = write_jsonl(&mut buf, records).unwrap();
        assert_eq!(n, 2);
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.lines().count(), 2);
        for line in s.lines() {
            let _: serde_json::Value =
                serde_json::from_str(line).expect("each line must be valid JSON");
        }
    }
}
