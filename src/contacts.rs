//! Parsing and searching vCard records returned by a CardDAV provider.
//!
//! Network access and credentials are intentionally kept out of this module so
//! provider records can be validated independently before exposing them via MCP.

use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Contact {
    pub uid: Option<String>,
    pub name: Option<String>,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organization: Option<String>,
    pub vcard: String,
}

pub fn parse_vcards(input: &str) -> Vec<Contact> {
    unfold_lines(input)
        .split("\nEND:VCARD")
        .filter_map(|record| {
            let record = record.trim();
            if record.is_empty() {
                return None;
            }
            let mut uid = None;
            let mut name = None;
            let mut organization = None;
            let mut emails = Vec::new();
            let mut phones = Vec::new();

            for line in record.lines() {
                let Some((property, value)) = line.split_once(':') else {
                    continue;
                };
                let property = property.split(';').next().unwrap_or(property);
                let value = unescape(value.trim());
                match property.to_ascii_uppercase().as_str() {
                    "UID" => uid = Some(value),
                    "FN" => name = Some(value),
                    "N" if name.is_none() => {
                        let parts = value.split(';').filter(|part| !part.is_empty());
                        let display = parts.collect::<Vec<_>>().join(" ");
                        if !display.is_empty() {
                            name = Some(display);
                        }
                    }
                    "ORG" => organization = Some(value.replace(';', " ")),
                    "EMAIL" if !value.is_empty() => emails.push(value),
                    "TEL" if !value.is_empty() => phones.push(value),
                    _ => {}
                }
            }
            Some(Contact {
                uid,
                name,
                emails,
                phones,
                organization,
                vcard: format!("{}\nEND:VCARD", record.trim_end_matches('\n')),
            })
        })
        .collect()
}

pub fn search(contacts: &[Contact], query: &str) -> Vec<Contact> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    contacts
        .iter()
        .filter(|contact| {
            contact.name.iter().chain(contact.organization.iter())
                .chain(contact.emails.iter()).chain(contact.phones.iter())
                .any(|value| value.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect()
}

fn unfold_lines(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::new();
    for line in normalized.split('\n') {
        if (line.starts_with(' ') || line.starts_with('\t')) && !output.is_empty() {
            output.pop();
            output.push_str(line.trim_start());
            output.push('\n');
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn unescape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n' | 'N') => output.push('\n'),
                Some(next) => output.push(next),
                None => output.push('\\'),
            }
        } else {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folded_vcard_and_escaped_values() {
        let cards = parse_vcards("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:42\r\nFN:Иван\\, Иванов\r\nEMAIL;TYPE=work:ivan@example.test\r\nTEL;TYPE=cell:+100\r\nORG:Домашний\\nофис\r\nEND:VCARD\r\n");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].name.as_deref(), Some("Иван, Иванов"));
        assert_eq!(cards[0].emails, ["ivan@example.test"]);
        assert_eq!(cards[0].phones, ["+100"]);
        assert_eq!(cards[0].organization.as_deref(), Some("Домашний\nофис"));
    }

    #[test]
    fn searches_name_email_organization_and_phone_case_insensitively() {
        let cards = parse_vcards("BEGIN:VCARD\nFN:Alice Smith\nEMAIL:alice@example.test\nORG:Example Corp\nTEL:+155500\nEND:VCARD\n");
        for query in ["ALICE", "example.test", "corp", "555"] {
            assert_eq!(search(&cards, query).len(), 1);
        }
        assert!(search(&cards, "  ").is_empty());
    }
}
