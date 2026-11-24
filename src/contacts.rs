//! Parsing and searching vCard records returned by a CardDAV provider.
//!
//! Credentials are supplied by the account resolver; provider records are
//! parsed here without exposing protocol details to MCP clients.

use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::Serialize;
use std::time::Duration;

use crate::error::{Result, YacliError};

const MAX_CARDDAV_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_VCARD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Contact {
    pub uid: Option<String>,
    pub name: Option<String>,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organization: Option<String>,
    pub href: Option<String>,
    pub etag: Option<String>,
    pub vcard: String,
}

/// Fetches the Yandex CardDAV address book using its documented addressbook
/// endpoint and an independently provisioned Contacts app password.
pub fn fetch_contacts(base_url: &str, email: &str, app_password: &str) -> Result<Vec<Contact>> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|_| YacliError::Config("invalid CardDAV base URL".into()))?;
    if base.scheme() != "https" || base.host_str() != Some("carddav.yandex.ru") {
        return Err(YacliError::Config(
            "CardDAV URL must use the official https://carddav.yandex.ru origin".into(),
        ));
    }
    let mut url = base;
    url.set_path(&format!(
        "/addressbook/{}/addressbook/",
        url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>()
    ));
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| {
            YacliError::UnsupportedOperation("CardDAV client initialization failed".into())
        })?;
    let body = r#"<?xml version="1.0" encoding="utf-8" ?>
<card:addressbook-query xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:prop><d:getetag/><card:address-data/></d:prop>
</card:addressbook-query>"#;
    let response = client
        .request(
            reqwest::Method::from_bytes(b"REPORT").expect("REPORT method"),
            url,
        )
        .basic_auth(email, Some(app_password))
        .header("Depth", "1")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(body)
        .send()
        .map_err(|_| YacliError::UnsupportedOperation("CardDAV request failed".into()))?;
    if response.status().is_redirection() {
        return Err(YacliError::UnsupportedOperation(
            "CardDAV redirected; refusing to forward credentials".into(),
        ));
    }
    if !response.status().is_success() {
        return Err(YacliError::UnsupportedOperation(format!(
            "CardDAV provider returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|_| YacliError::UnsupportedOperation("CardDAV response read failed".into()))?;
    if bytes.len() > MAX_CARDDAV_RESPONSE_BYTES {
        return Err(YacliError::UnsupportedOperation(
            "CardDAV response exceeds 8 MiB limit".into(),
        ));
    }
    let xml = String::from_utf8(bytes.to_vec()).map_err(|_| {
        YacliError::UnsupportedOperation("CardDAV response is not UTF-8 XML".into())
    })?;
    parse_carddav_multistatus(&xml)
}

pub fn create_contact(
    base_url: &str,
    email: &str,
    app_password: &str,
    vcard: &str,
) -> Result<String> {
    validate_vcard(vcard)?;
    let collection = addressbook_url(base_url, email)?;
    let uid = format!("yacli-{:032x}", rand::random::<u128>());
    let card = with_uid(vcard, &uid)?;
    let target = collection
        .join(&format!("{uid}.vcf"))
        .map_err(|_| YacliError::Config("invalid CardDAV resource URL".into()))?;
    let response = carddav_client()?
        .put(target)
        .basic_auth(email, Some(app_password))
        .header("If-None-Match", "*")
        .header("Content-Type", "text/vcard; charset=utf-8")
        .body(card)
        .send()
        .map_err(|_| YacliError::UnsupportedOperation("CardDAV create request failed".into()))?;
    ensure_write_success(response.status().as_u16(), "create")?;
    Ok(uid)
}

pub fn update_contact(
    base_url: &str,
    email: &str,
    app_password: &str,
    href: &str,
    etag: &str,
    vcard: &str,
) -> Result<()> {
    validate_vcard(vcard)?;
    validate_etag(etag)?;
    let target = safe_resource_url(base_url, href)?;
    let response = carddav_client()?
        .put(target)
        .basic_auth(email, Some(app_password))
        .header("If-Match", etag)
        .header("Content-Type", "text/vcard; charset=utf-8")
        .body(vcard.to_string())
        .send()
        .map_err(|_| YacliError::UnsupportedOperation("CardDAV update request failed".into()))?;
    ensure_write_success(response.status().as_u16(), "update")
}

pub fn delete_contact(
    base_url: &str,
    email: &str,
    app_password: &str,
    href: &str,
    etag: &str,
) -> Result<()> {
    validate_etag(etag)?;
    let target = safe_resource_url(base_url, href)?;
    let response = carddav_client()?
        .delete(target)
        .basic_auth(email, Some(app_password))
        .header("If-Match", etag)
        .send()
        .map_err(|_| YacliError::UnsupportedOperation("CardDAV delete request failed".into()))?;
    ensure_write_success(response.status().as_u16(), "delete")
}

fn carddav_client() -> Result<Client> {
    Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| {
            YacliError::UnsupportedOperation("CardDAV client initialization failed".into())
        })
}

fn addressbook_url(base_url: &str, email: &str) -> Result<reqwest::Url> {
    let mut base = reqwest::Url::parse(base_url)
        .map_err(|_| YacliError::Config("invalid CardDAV base URL".into()))?;
    if base.scheme() != "https" || base.host_str() != Some("carddav.yandex.ru") {
        return Err(YacliError::Config(
            "CardDAV URL must use the official https://carddav.yandex.ru origin".into(),
        ));
    }
    base.set_path(&format!(
        "/addressbook/{}/addressbook/",
        url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>()
    ));
    Ok(base)
}

fn safe_resource_url(base_url: &str, href: &str) -> Result<reqwest::Url> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|_| YacliError::Config("invalid CardDAV base URL".into()))?;
    if base.scheme() != "https" || base.host_str() != Some("carddav.yandex.ru") {
        return Err(YacliError::Config(
            "CardDAV URL must use the official https://carddav.yandex.ru origin".into(),
        ));
    }
    if !href.starts_with("/addressbook/")
        || href.contains('%')
        || href.contains('\\')
        || href.split('/').any(|part| part == "." || part == "..")
    {
        return Err(YacliError::Validation(
            "CardDAV href is outside the expected addressbook path".into(),
        ));
    }
    let target = base
        .join(href)
        .map_err(|_| YacliError::Validation("invalid CardDAV href".into()))?;
    if target.origin() != base.origin()
        || target.username() != ""
        || target.password().is_some()
        || target.path().contains("..")
    {
        return Err(YacliError::Validation(
            "CardDAV href escapes the configured origin".into(),
        ));
    }
    Ok(target)
}

fn validate_vcard(vcard: &str) -> Result<()> {
    if vcard.len() > MAX_VCARD_BYTES
        || !vcard.to_ascii_uppercase().contains("BEGIN:VCARD")
        || !vcard.to_ascii_uppercase().contains("END:VCARD")
        || vcard.chars().any(|ch| ch == '\0')
    {
        return Err(YacliError::Validation(
            "vCard is invalid or exceeds 1 MiB".into(),
        ));
    }
    Ok(())
}

fn validate_etag(etag: &str) -> Result<()> {
    if etag.is_empty()
        || etag.len() > 512
        || etag == "*"
        || etag.starts_with("W/")
        || etag.chars().any(char::is_control)
    {
        return Err(YacliError::Validation(
            "CardDAV operation requires a strong ETag".into(),
        ));
    }
    Ok(())
}

fn with_uid(vcard: &str, uid: &str) -> Result<String> {
    let mut lines = vcard
        .replace("\r\n", "\n")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines
        .iter()
        .any(|line| line.to_ascii_uppercase().starts_with("UID:"))
    {
        return Err(YacliError::Validation(
            "new contact must not supply its own UID".into(),
        ));
    }
    let begin = lines
        .iter()
        .position(|line| line.eq_ignore_ascii_case("BEGIN:VCARD"))
        .ok_or_else(|| YacliError::Validation("vCard BEGIN:VCARD is required".into()))?;
    lines.insert(begin + 1, format!("UID:{uid}"));
    Ok(format!("{}\r\n", lines.join("\r\n")))
}

fn ensure_write_success(status: u16, action: &str) -> Result<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    if status == 412 || status == 409 {
        return Err(YacliError::Api(format!(
            "CardDAV {action} conflict; refresh the contact and review again"
        )));
    }
    Err(YacliError::UnsupportedOperation(format!(
        "CardDAV {action} returned HTTP {status}"
    )))
}

fn parse_carddav_multistatus(xml: &str) -> Result<Vec<Contact>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut vcard = String::new();
    let mut cards = Vec::new();
    let mut in_address_data = false;
    let mut capture: Option<&'static str> = None;
    let mut current_href = None;
    let mut current_etag = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event))
                if event.local_name().as_ref() == b"address-data" =>
            {
                in_address_data = true
            }
            Ok(quick_xml::events::Event::Start(event))
                if event.local_name().as_ref() == b"href" =>
            {
                capture = Some("href")
            }
            Ok(quick_xml::events::Event::Start(event))
                if event.local_name().as_ref() == b"getetag" =>
            {
                capture = Some("etag")
            }
            Ok(quick_xml::events::Event::End(event))
                if event.local_name().as_ref() == b"address-data" =>
            {
                in_address_data = false;
                for mut card in parse_vcards(&vcard) {
                    card.href = current_href.take();
                    card.etag = current_etag.take();
                    cards.push(card);
                }
                vcard.clear();
            }
            Ok(quick_xml::events::Event::End(event))
                if event.local_name().as_ref() == b"href"
                    || event.local_name().as_ref() == b"getetag" =>
            {
                capture = None
            }
            Ok(quick_xml::events::Event::Text(event)) if in_address_data => {
                if let Ok(text) = std::str::from_utf8(event.as_ref())
                    && let Ok(text) = quick_xml::escape::unescape(text)
                {
                    vcard.push_str(&text);
                }
            }
            Ok(quick_xml::events::Event::Text(event)) if capture.is_some() => {
                if let Ok(raw) = std::str::from_utf8(event.as_ref())
                    && let Ok(text) = quick_xml::escape::unescape(raw)
                {
                    match capture {
                        Some("href") => current_href = Some(text.into_owned()),
                        Some("etag") => current_etag = Some(text.into_owned()),
                        _ => {}
                    }
                }
            }
            Ok(quick_xml::events::Event::CData(event)) if in_address_data => {
                if let Ok(text) = std::str::from_utf8(event.as_ref()) {
                    vcard.push_str(text);
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => {
                return Err(YacliError::UnsupportedOperation(
                    "invalid CardDAV XML response".into(),
                ));
            }
            _ => {}
        }
    }
    Ok(cards)
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
                href: None,
                etag: None,
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
            contact
                .name
                .iter()
                .chain(contact.organization.iter())
                .chain(contact.emails.iter())
                .chain(contact.phones.iter())
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
        let cards = parse_vcards(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:42\r\nFN:Иван\\, Иванов\r\nEMAIL;TYPE=work:ivan@example.test\r\nTEL;TYPE=cell:+100\r\nORG:Домашний\\nофис\r\nEND:VCARD\r\n",
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].name.as_deref(), Some("Иван, Иванов"));
        assert_eq!(cards[0].emails, ["ivan@example.test"]);
        assert_eq!(cards[0].phones, ["+100"]);
        assert_eq!(cards[0].organization.as_deref(), Some("Домашний\nофис"));
    }

    #[test]
    fn searches_name_email_organization_and_phone_case_insensitively() {
        let cards = parse_vcards(
            "BEGIN:VCARD\nFN:Alice Smith\nEMAIL:alice@example.test\nORG:Example Corp\nTEL:+155500\nEND:VCARD\n",
        );
        for query in ["ALICE", "example.test", "corp", "555"] {
            assert_eq!(search(&cards, query).len(), 1);
        }
        assert!(search(&cards, "  ").is_empty());
    }

    #[test]
    fn rejects_non_yandex_and_non_https_origins_before_request() {
        for origin in [
            "http://carddav.yandex.ru",
            "https://example.test",
            "https://carddav.yandex.ru.evil.test",
        ] {
            assert!(fetch_contacts(origin, "owner@example.test", "synthetic-secret").is_err());
        }
    }

    #[test]
    fn parses_carddav_multistatus_address_data() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/addressbook/user/contact.vcf</d:href><d:propstat><d:prop><d:getetag>"tag-1"</d:getetag><card:address-data>BEGIN:VCARD&#10;VERSION:3.0&#10;UID:9&#10;FN:Тест&#10;EMAIL:test@example.test&#10;END:VCARD</card:address-data></d:prop></d:propstat></d:response></d:multistatus>"#;
        let cards = parse_carddav_multistatus(xml).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].uid.as_deref(), Some("9"));
        assert_eq!(cards[0].name.as_deref(), Some("Тест"));
        assert_eq!(cards[0].emails, ["test@example.test"]);
        assert_eq!(
            cards[0].href.as_deref(),
            Some("/addressbook/user/contact.vcf")
        );
        assert_eq!(cards[0].etag.as_deref(), Some("\"tag-1\""));
    }

    #[test]
    fn contact_mutations_require_safe_href_etag_and_vcard() {
        assert!(safe_resource_url("https://carddav.yandex.ru", "/addressbook/u/a.vcf").is_ok());
        for href in [
            "https://evil.test/a.vcf",
            "//evil.test/a.vcf",
            "/addressbook/../private",
        ] {
            assert!(safe_resource_url("https://carddav.yandex.ru", href).is_err());
        }
        assert!(validate_etag("\"etag-1\"").is_ok());
        assert!(validate_etag("*").is_err());
        assert!(validate_etag("W/\"weak\"").is_err());
        let card = with_uid("BEGIN:VCARD\nVERSION:3.0\nFN:Name\nEND:VCARD", "new-id").unwrap();
        assert!(card.contains("UID:new-id"));
        assert!(with_uid("BEGIN:VCARD\nUID:existing\nEND:VCARD", "new-id").is_err());
    }
}
