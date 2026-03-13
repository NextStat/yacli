use std::time::Duration;

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;
use rand::random;
use reqwest::Method;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Serialize;
use url::Url;

use crate::error::{Result, YacliError};

const CALDAV_TIMEOUT_SECS: u64 = 20;
const DEFAULT_EVENT_WINDOW_DAYS: u64 = 30;

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct CalendarCollection {
    pub id: String,
    pub name: String,
    pub href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct CalendarEventWindow {
    pub from: String,
    pub to: String,
    pub limit: usize,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct CalendarEvent {
    pub calendar_id: String,
    pub calendar_name: String,
    pub href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub all_day: bool,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct CalendarInvite {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub all_day: bool,
}

#[derive(Clone, Debug)]
pub struct CalendarEventsRequest {
    pub calendar: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct CalendarCreateRequest {
    pub calendar: String,
    pub summary: String,
    pub start: String,
    pub end: String,
    pub description: Option<String>,
    pub location: Option<String>,
}

pub fn calendar_create_request_from_invites(
    calendar: &str,
    invites: &[CalendarInvite],
    event_index: usize,
    command_name: &str,
) -> Result<(CalendarCreateRequest, CalendarInvite)> {
    if event_index == 0 {
        return Err(YacliError::Validation(format!(
            "{command_name} --event-index must be greater than zero"
        )));
    }
    if invites.is_empty() {
        return Err(YacliError::Validation(format!(
            "{command_name} attachment does not contain any VEVENT entries"
        )));
    }

    let Some(invite) = invites.get(event_index - 1).cloned() else {
        return Err(YacliError::Validation(format!(
            "{command_name} --event-index {event_index} is out of range; attachment contains {} VEVENT entries",
            invites.len()
        )));
    };
    let summary = invite.summary.clone().ok_or_else(|| {
        YacliError::Validation(format!("{command_name} selected VEVENT is missing SUMMARY"))
    })?;
    let start = invite.start.clone().ok_or_else(|| {
        YacliError::Validation(format!("{command_name} selected VEVENT is missing DTSTART"))
    })?;
    let end = invite.end.clone().ok_or_else(|| {
        YacliError::Validation(format!("{command_name} selected VEVENT is missing DTEND"))
    })?;

    Ok((
        CalendarCreateRequest {
            calendar: calendar.to_string(),
            summary,
            start,
            end,
            description: invite.description.clone(),
            location: invite.location.clone(),
        },
        invite,
    ))
}

pub fn parse_event_window(
    from: Option<&str>,
    to: Option<&str>,
    limit: usize,
) -> Result<CalendarEventWindow> {
    if limit == 0 {
        return Err(YacliError::Validation(
            "calendar events --limit должен быть больше нуля".to_string(),
        ));
    }
    if limit > 100 {
        return Err(YacliError::Validation(
            "calendar events --limit не должен быть больше 100".to_string(),
        ));
    }

    let now = Utc::now();
    let default_from = now;
    let default_to = now
        .checked_add_days(Days::new(DEFAULT_EVENT_WINDOW_DAYS))
        .ok_or_else(|| {
            YacliError::Validation("failed to compute default calendar time window".to_string())
        })?;

    let from = match from {
        Some(value) => parse_time_boundary(value, "calendar events <FROM>")?,
        None => default_from,
    };
    let to = match to {
        Some(value) => parse_time_boundary(value, "calendar events <TO>")?,
        None => default_to,
    };

    if to <= from {
        return Err(YacliError::Validation(
            "calendar events <TO> должен быть позже <FROM>".to_string(),
        ));
    }

    Ok(CalendarEventWindow {
        from: from.to_rfc3339_opts(SecondsFormat::Secs, true),
        to: to.to_rfc3339_opts(SecondsFormat::Secs, true),
        limit,
    })
}

pub fn list_calendars(
    base_url: &str,
    account: &str,
    app_password: &str,
) -> Result<Vec<CalendarCollection>> {
    let client = CaldavClient::new(base_url, account, app_password)?;
    client.list_calendars()
}

pub fn list_calendar_events(
    base_url: &str,
    account: &str,
    app_password: &str,
    request: CalendarEventsRequest,
) -> Result<(CalendarCollection, CalendarEventWindow, Vec<CalendarEvent>)> {
    let client = CaldavClient::new(base_url, account, app_password)?;
    let window = CalendarEventWindow {
        from: request.from.to_rfc3339_opts(SecondsFormat::Secs, true),
        to: request.to.to_rfc3339_opts(SecondsFormat::Secs, true),
        limit: request.limit,
    };
    let (calendar, events) = client.list_events(&request.calendar, &request)?;
    Ok((calendar, window, events))
}

pub fn create_calendar_event(
    base_url: &str,
    account: &str,
    app_password: &str,
    request: CalendarCreateRequest,
) -> Result<(CalendarCollection, CalendarEvent)> {
    let client = CaldavClient::new(base_url, account, app_password)?;
    client.create_event(request)
}

pub fn delete_calendar_event(
    base_url: &str,
    account: &str,
    app_password: &str,
    calendar_ref: &str,
    uid: &str,
) -> Result<(CalendarCollection, CalendarEvent)> {
    let client = CaldavClient::new(base_url, account, app_password)?;
    client.delete_event(calendar_ref, uid)
}

pub fn parse_calendar_invites(calendar_data: &str) -> Result<Vec<CalendarInvite>> {
    Ok(parse_ical_events(calendar_data)?
        .into_iter()
        .map(|event| CalendarInvite {
            uid: event.uid,
            summary: event.summary,
            start: event.start,
            end: event.end,
            description: event.description,
            location: event.location,
            status: event.status,
            all_day: event.all_day,
        })
        .collect())
}

enum ParsedCalendarBoundary {
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
}

struct ParsedCalendarWriteWindow {
    start: ParsedCalendarBoundary,
    end: ParsedCalendarBoundary,
    normalized_start: String,
    normalized_end: String,
    all_day: bool,
}

struct CaldavClient {
    http: Client,
    base_url: Url,
    account: String,
    app_password: String,
}

impl CaldavClient {
    fn new(base_url: &str, account: &str, app_password: &str) -> Result<Self> {
        let base_url = Url::parse(base_url)
            .map_err(|err| YacliError::Config(format!("invalid CalDAV base URL: {err}")))?;
        let http = Client::builder()
            .user_agent(format!("yacli/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(CALDAV_TIMEOUT_SECS))
            .build()?;

        Ok(Self {
            http,
            base_url,
            account: account.to_string(),
            app_password: app_password.to_string(),
        })
    }

    fn list_calendars(&self) -> Result<Vec<CalendarCollection>> {
        let principal_url = self.current_user_principal()?;
        let home_set_url = self.calendar_home_set(&principal_url)?;
        let xml = self.send_xml(
            Method::from_bytes(b"PROPFIND")
                .map_err(|err| YacliError::Config(format!("invalid PROPFIND method: {err}")))?,
            &home_set_url,
            Some("1"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <c:calendar-description/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>"#,
            "CalDAV calendar discovery",
        )?;

        let mut calendars = parse_dav_responses(&xml)?
            .into_iter()
            .filter(|response| response.resourcetypes.iter().any(|kind| kind == "calendar"))
            .filter_map(|response| {
                let href = response.href?;
                let id = collection_id_from_href(&href)?;
                let name = response.displayname.unwrap_or_else(|| id.clone());
                Some(CalendarCollection {
                    id,
                    name,
                    href,
                    description: response.calendar_description,
                })
            })
            .collect::<Vec<_>>();

        calendars.sort_by(|left, right| left.id.cmp(&right.id).then(left.name.cmp(&right.name)));
        Ok(calendars)
    }

    fn find_calendar(&self, calendar_ref: &str) -> Result<CalendarCollection> {
        self.list_calendars()?
            .into_iter()
            .find(|calendar| {
                calendar.id == calendar_ref
                    || calendar.name == calendar_ref
                    || calendar.href == calendar_ref
            })
            .ok_or_else(|| {
                YacliError::Validation(format!(
                    "calendar `{calendar_ref}` not found; run `yacli calendar calendars` to inspect available ids"
                ))
            })
    }

    fn list_events(
        &self,
        calendar_ref: &str,
        request: &CalendarEventsRequest,
    ) -> Result<(CalendarCollection, Vec<CalendarEvent>)> {
        let calendar = self.find_calendar(calendar_ref)?;

        let xml = self.send_xml(
            Method::from_bytes(b"REPORT")
                .map_err(|err| YacliError::Config(format!("invalid REPORT method: {err}")))?,
            &self.resolve_href(&calendar.href)?,
            Some("1"),
            &build_calendar_query_xml(request),
            "CalDAV calendar-query REPORT",
        )?;

        let mut events = Vec::new();
        for response in parse_dav_responses(&xml)? {
            let Some(calendar_data) = response.calendar_data.as_deref() else {
                continue;
            };
            let href = response.href.unwrap_or_else(|| calendar.href.clone());
            for event in parse_ical_events(calendar_data)? {
                events.push(CalendarEvent {
                    calendar_id: calendar.id.clone(),
                    calendar_name: calendar.name.clone(),
                    href: href.clone(),
                    uid: event.uid,
                    summary: event.summary,
                    start: event.start,
                    end: event.end,
                    description: event.description,
                    location: event.location,
                    status: event.status,
                    etag: response.getetag.clone(),
                    all_day: event.all_day,
                });
            }
        }

        events.sort_by(|left, right| {
            left.start
                .cmp(&right.start)
                .then(left.summary.cmp(&right.summary))
                .then(left.uid.cmp(&right.uid))
        });
        if events.len() > request.limit {
            events.truncate(request.limit);
        }

        Ok((calendar, events))
    }

    fn create_event(
        &self,
        request: CalendarCreateRequest,
    ) -> Result<(CalendarCollection, CalendarEvent)> {
        let calendar = self.find_calendar(&request.calendar)?;
        let window = parse_create_event_window(&request.start, &request.end)?;

        let summary = request.summary.trim();
        if summary.is_empty() {
            return Err(YacliError::Validation(
                "calendar create <SUMMARY> не должен быть пустым".to_string(),
            ));
        }

        let uid = generate_calendar_uid();
        let event_href = format!(
            "{}{}.ics",
            calendar.href,
            sanitize_uid_for_href(uid.as_str())
        );
        let event_url = self.resolve_href(&event_href)?;
        let ical = build_calendar_event_ical(
            &uid,
            summary,
            request.description.as_deref(),
            request.location.as_deref(),
            &window,
        );
        let put_etag = self.send_calendar_put(&event_url, &ical, "CalDAV event create")?;
        let mut created = self.find_event_by_uid(&calendar, &uid)?;
        if created.etag.is_none() {
            created.etag = put_etag;
        }
        if created.summary.is_none() {
            created.summary = Some(summary.to_string());
        }
        if created.description.is_none() {
            created.description = request.description;
        }
        if created.location.is_none() {
            created.location = request.location;
        }
        if created.start.is_none() {
            created.start = Some(window.normalized_start);
        }
        if created.end.is_none() {
            created.end = Some(window.normalized_end);
        }
        if created.status.is_none() {
            created.status = Some("CONFIRMED".to_string());
        }
        created.all_day = window.all_day;

        Ok((calendar, created))
    }

    fn delete_event(
        &self,
        calendar_ref: &str,
        uid: &str,
    ) -> Result<(CalendarCollection, CalendarEvent)> {
        let calendar = self.find_calendar(calendar_ref)?;
        let event = self.find_event_by_uid(&calendar, uid)?;
        let url = self.resolve_href(&event.href)?;
        self.send_empty(
            Method::DELETE,
            &url,
            event.etag.as_deref(),
            "CalDAV event delete",
        )?;
        Ok((calendar, event))
    }

    fn find_event_by_uid(&self, calendar: &CalendarCollection, uid: &str) -> Result<CalendarEvent> {
        let xml = self.send_xml(
            Method::from_bytes(b"REPORT")
                .map_err(|err| YacliError::Config(format!("invalid REPORT method: {err}")))?,
            &self.resolve_href(&calendar.href)?,
            Some("1"),
            &build_calendar_uid_query_xml(uid),
            "CalDAV event lookup REPORT",
        )?;

        let mut matches = Vec::new();
        for response in parse_dav_responses(&xml)? {
            let Some(calendar_data) = response.calendar_data.as_deref() else {
                continue;
            };
            let href = response.href.unwrap_or_else(|| calendar.href.clone());
            for event in parse_ical_events(calendar_data)? {
                if event.uid.as_deref() != Some(uid) {
                    continue;
                }
                matches.push(CalendarEvent {
                    calendar_id: calendar.id.clone(),
                    calendar_name: calendar.name.clone(),
                    href: href.clone(),
                    uid: event.uid,
                    summary: event.summary,
                    start: event.start,
                    end: event.end,
                    description: event.description,
                    location: event.location,
                    status: event.status,
                    etag: response.getetag.clone(),
                    all_day: event.all_day,
                });
            }
        }

        match matches.len() {
            0 => Err(YacliError::Validation(format!(
                "calendar event with uid `{uid}` not found in calendar `{}`",
                calendar.id
            ))),
            1 => Ok(matches.remove(0)),
            _ => Err(YacliError::Integrity(format!(
                "multiple calendar events with uid `{uid}` were returned for calendar `{}`",
                calendar.id
            ))),
        }
    }

    fn current_user_principal(&self) -> Result<Url> {
        let xml = self.send_xml(
            Method::from_bytes(b"PROPFIND")
                .map_err(|err| YacliError::Config(format!("invalid PROPFIND method: {err}")))?,
            &self.base_url,
            Some("0"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:current-user-principal/>
  </d:prop>
</d:propfind>"#,
            "CalDAV current-user-principal discovery",
        )?;

        let href = parse_dav_responses(&xml)?
            .into_iter()
            .find_map(|response| response.current_user_principal)
            .ok_or_else(|| {
                YacliError::Api("CalDAV server did not return current-user-principal".to_string())
            })?;
        self.resolve_href(&href)
    }

    fn calendar_home_set(&self, principal_url: &Url) -> Result<Url> {
        let xml = self.send_xml(
            Method::from_bytes(b"PROPFIND")
                .map_err(|err| YacliError::Config(format!("invalid PROPFIND method: {err}")))?,
            principal_url,
            Some("0"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <c:calendar-home-set/>
  </d:prop>
</d:propfind>"#,
            "CalDAV calendar-home-set discovery",
        )?;

        let href = parse_dav_responses(&xml)?
            .into_iter()
            .find_map(|response| response.calendar_home_set)
            .ok_or_else(|| {
                YacliError::Api("CalDAV server did not return calendar-home-set".to_string())
            })?;
        self.resolve_href(&href)
    }

    fn send_xml(
        &self,
        method: Method,
        url: &Url,
        depth: Option<&str>,
        body: &str,
        action: &str,
    ) -> Result<String> {
        let mut request = self
            .http
            .request(method, url.clone())
            .basic_auth(&self.account, Some(&self.app_password))
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.to_string());

        if let Some(depth) = depth {
            request = request.header("Depth", depth);
        }

        let response = request.send()?;
        let status = response.status();
        let body = response.text()?;

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(YacliError::Auth(format!(
                "{action} failed with status {}. Check the Yandex app password and account.email value.",
                status.as_u16()
            )));
        }
        if !status.is_success() && status.as_u16() != 207 {
            return Err(YacliError::Api(format!(
                "{action} failed with status {}: {}",
                status.as_u16(),
                truncate_body(&body)
            )));
        }

        Ok(body)
    }

    fn send_calendar_put(&self, url: &Url, body: &str, action: &str) -> Result<Option<String>> {
        let response = self
            .http
            .request(Method::PUT, url.clone())
            .basic_auth(&self.account, Some(&self.app_password))
            .header("Content-Type", "text/calendar; charset=utf-8")
            .header("If-None-Match", "*")
            .body(body.to_string())
            .send()?;

        let status = response.status();
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let response_body = response.text()?;

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(YacliError::Auth(format!(
                "{action} failed with status {}. Check the Yandex app password and account.email value.",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(YacliError::Api(format!(
                "{action} failed with status {}: {}",
                status.as_u16(),
                truncate_body(&response_body)
            )));
        }

        Ok(etag)
    }

    fn send_empty(
        &self,
        method: Method,
        url: &Url,
        if_match: Option<&str>,
        action: &str,
    ) -> Result<()> {
        let mut request = self
            .http
            .request(method, url.clone())
            .basic_auth(&self.account, Some(&self.app_password));
        if let Some(etag) = if_match {
            request = request.header("If-Match", etag);
        }

        let response = request.send()?;
        let status = response.status();
        let body = response.text()?;

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(YacliError::Auth(format!(
                "{action} failed with status {}. Check the Yandex app password and account.email value.",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(YacliError::Api(format!(
                "{action} failed with status {}: {}",
                status.as_u16(),
                truncate_body(&body)
            )));
        }

        Ok(())
    }

    fn resolve_href(&self, href: &str) -> Result<Url> {
        self.base_url
            .join(href)
            .map_err(|err| YacliError::Config(format!("invalid CalDAV href `{href}`: {err}")))
    }
}

#[derive(Default, Clone)]
struct DavResponse {
    href: Option<String>,
    displayname: Option<String>,
    current_user_principal: Option<String>,
    calendar_home_set: Option<String>,
    calendar_description: Option<String>,
    getetag: Option<String>,
    calendar_data: Option<String>,
    resourcetypes: Vec<String>,
}

fn parse_dav_responses(xml: &str) -> Result<Vec<DavResponse>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut responses = Vec::new();
    let mut current: Option<DavResponse> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref()).to_string();
                if name == "response" {
                    current = Some(DavResponse::default());
                }
                if let Some(response) = current.as_mut()
                    && path_ends_with(&stack, &["response", "propstat", "prop", "resourcetype"])
                {
                    response.resourcetypes.push(name.clone());
                }
                stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref()).to_string();
                if let Some(response) = current.as_mut()
                    && path_ends_with(&stack, &["response", "propstat", "prop", "resourcetype"])
                {
                    response.resourcetypes.push(name);
                }
            }
            Ok(Event::Text(event)) => {
                let text = event
                    .unescape()
                    .map_err(|err| {
                        YacliError::Serialization(format!(
                            "failed to decode CalDAV XML text: {err}"
                        ))
                    })?
                    .into_owned();
                apply_dav_text(&stack, current.as_mut(), text);
            }
            Ok(Event::CData(event)) => {
                let text = String::from_utf8_lossy(event.as_ref()).into_owned();
                apply_dav_text(&stack, current.as_mut(), text);
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref()).to_string();
                if name == "response"
                    && let Some(response) = current.take()
                {
                    responses.push(response);
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(YacliError::Serialization(format!(
                    "failed to parse CalDAV XML response: {err}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(responses)
}

fn apply_dav_text(stack: &[String], current: Option<&mut DavResponse>, text: String) {
    let Some(response) = current else {
        return;
    };
    if text.is_empty() {
        return;
    }

    if path_ends_with(
        stack,
        &[
            "response",
            "propstat",
            "prop",
            "current-user-principal",
            "href",
        ],
    ) {
        response.current_user_principal.get_or_insert(text);
    } else if path_ends_with(
        stack,
        &["response", "propstat", "prop", "calendar-home-set", "href"],
    ) {
        response.calendar_home_set.get_or_insert(text);
    } else if path_ends_with(stack, &["response", "propstat", "prop", "displayname"]) {
        response.displayname.get_or_insert(text);
    } else if path_ends_with(
        stack,
        &["response", "propstat", "prop", "calendar-description"],
    ) {
        response.calendar_description.get_or_insert(text);
    } else if path_ends_with(stack, &["response", "propstat", "prop", "getetag"]) {
        response.getetag.get_or_insert(text);
    } else if path_ends_with(stack, &["response", "propstat", "prop", "calendar-data"]) {
        response.calendar_data.get_or_insert(text);
    } else if path_ends_with(stack, &["response", "href"]) {
        response.href.get_or_insert(text);
    }
}

fn path_ends_with(stack: &[String], tail: &[&str]) -> bool {
    if stack.len() < tail.len() {
        return false;
    }
    stack[stack.len() - tail.len()..]
        .iter()
        .map(String::as_str)
        .eq(tail.iter().copied())
}

fn local_name(name: &[u8]) -> &str {
    let value = std::str::from_utf8(name).unwrap_or_default();
    value.rsplit(':').next().unwrap_or(value)
}

fn truncate_body(body: &str) -> String {
    const MAX_BODY_CHARS: usize = 240;
    if body.chars().count() <= MAX_BODY_CHARS {
        return body.to_string();
    }
    let truncated = body.chars().take(MAX_BODY_CHARS).collect::<String>();
    format!("{truncated}...")
}

fn build_calendar_query_xml(request: &CalendarEventsRequest) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data>
      <c:comp name="VCALENDAR">
        <c:comp name="VEVENT">
          <c:prop name="UID"/>
          <c:prop name="SUMMARY"/>
          <c:prop name="DTSTART"/>
          <c:prop name="DTEND"/>
          <c:prop name="DESCRIPTION"/>
          <c:prop name="LOCATION"/>
          <c:prop name="STATUS"/>
        </c:comp>
      </c:comp>
    </c:calendar-data>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{from}" end="{to}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
        from = request.from.format("%Y%m%dT%H%M%SZ"),
        to = request.to.format("%Y%m%dT%H%M%SZ"),
    )
}

fn build_calendar_uid_query_xml(uid: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data>
      <c:comp name="VCALENDAR">
        <c:comp name="VEVENT">
          <c:prop name="UID"/>
          <c:prop name="SUMMARY"/>
          <c:prop name="DTSTART"/>
          <c:prop name="DTEND"/>
          <c:prop name="DESCRIPTION"/>
          <c:prop name="LOCATION"/>
          <c:prop name="STATUS"/>
        </c:comp>
      </c:comp>
    </c:calendar-data>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:prop-filter name="UID">
          <c:text-match collation="i;octet">{uid}</c:text-match>
        </c:prop-filter>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
        uid = escape_xml_text(uid)
    )
}

fn parse_create_event_window(start: &str, end: &str) -> Result<ParsedCalendarWriteWindow> {
    let start = parse_calendar_boundary(start, "calendar create <START>")?;
    let end = parse_calendar_boundary(end, "calendar create <END>")?;

    match (start, end) {
        (ParsedCalendarBoundary::Date(start), ParsedCalendarBoundary::Date(end)) => {
            if end <= start {
                return Err(YacliError::Validation(
                    "calendar create <END> должен быть позже <START>".to_string(),
                ));
            }
            Ok(ParsedCalendarWriteWindow {
                start: ParsedCalendarBoundary::Date(start),
                end: ParsedCalendarBoundary::Date(end),
                normalized_start: start.format("%Y-%m-%d").to_string(),
                normalized_end: end.format("%Y-%m-%d").to_string(),
                all_day: true,
            })
        }
        (ParsedCalendarBoundary::DateTime(start), ParsedCalendarBoundary::DateTime(end)) => {
            if end <= start {
                return Err(YacliError::Validation(
                    "calendar create <END> должен быть позже <START>".to_string(),
                ));
            }
            Ok(ParsedCalendarWriteWindow {
                start: ParsedCalendarBoundary::DateTime(start),
                end: ParsedCalendarBoundary::DateTime(end),
                normalized_start: start.to_rfc3339_opts(SecondsFormat::Secs, true),
                normalized_end: end.to_rfc3339_opts(SecondsFormat::Secs, true),
                all_day: false,
            })
        }
        _ => Err(YacliError::Validation(
            "calendar create requires both --start and --end to use the same format: either YYYY-MM-DD for all-day events or RFC3339 for timed events".to_string(),
        )),
    }
}

fn parse_calendar_boundary(value: &str, flag_name: &str) -> Result<ParsedCalendarBoundary> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(ParsedCalendarBoundary::Date(date));
    }

    DateTime::parse_from_rfc3339(value)
        .map(|value| ParsedCalendarBoundary::DateTime(value.with_timezone(&Utc)))
        .map_err(|_| {
            YacliError::Validation(format!(
                "{flag_name} must use YYYY-MM-DD or RFC3339, got `{value}`"
            ))
        })
}

fn build_calendar_event_ical(
    uid: &str,
    summary: &str,
    description: Option<&str>,
    location: Option<&str>,
    window: &ParsedCalendarWriteWindow,
) -> String {
    let dtstamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dtstart = match &window.start {
        ParsedCalendarBoundary::Date(value) => {
            format!("DTSTART;VALUE=DATE:{}", value.format("%Y%m%d"))
        }
        ParsedCalendarBoundary::DateTime(value) => {
            format!("DTSTART:{}", value.format("%Y%m%dT%H%M%SZ"))
        }
    };
    let dtend = match &window.end {
        ParsedCalendarBoundary::Date(value) => {
            format!("DTEND;VALUE=DATE:{}", value.format("%Y%m%d"))
        }
        ParsedCalendarBoundary::DateTime(value) => {
            format!("DTEND:{}", value.format("%Y%m%dT%H%M%SZ"))
        }
    };

    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//NextStat//yacli//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!("UID:{uid}"),
        format!("DTSTAMP:{dtstamp}"),
        format!("SUMMARY:{}", escape_ical_text(summary)),
        dtstart,
        dtend,
        "STATUS:CONFIRMED".to_string(),
    ];
    if let Some(description) = description.filter(|value| !value.trim().is_empty()) {
        lines.push(format!(
            "DESCRIPTION:{}",
            escape_ical_text(description.trim())
        ));
    }
    if let Some(location) = location.filter(|value| !value.trim().is_empty()) {
        lines.push(format!("LOCATION:{}", escape_ical_text(location.trim())));
    }
    lines.push("END:VEVENT".to_string());
    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n")
}

fn collection_id_from_href(href: &str) -> Option<String> {
    href.trim_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(ToString::to_string)
}

fn generate_calendar_uid() -> String {
    format!(
        "yacli-{}-{:016x}@nextstat.dev",
        Utc::now().timestamp_micros(),
        random::<u64>()
    )
}

fn sanitize_uid_for_href(uid: &str) -> String {
    uid.chars()
        .map(|char| match char {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => char,
            _ => '-',
        })
        .collect()
}

#[derive(Default)]
struct RawCalendarEvent {
    uid: Option<String>,
    summary: Option<String>,
    start: Option<String>,
    end: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: Option<String>,
    all_day: bool,
}

type IcalProperty = (String, String, Vec<(String, String)>);

fn parse_ical_events(calendar_data: &str) -> Result<Vec<RawCalendarEvent>> {
    let unfolded = unfold_ical_lines(calendar_data);
    let mut events = Vec::new();
    let mut current: Option<RawCalendarEvent> = None;

    for line in unfolded {
        match line.as_str() {
            "BEGIN:VEVENT" => current = Some(RawCalendarEvent::default()),
            "END:VEVENT" => {
                if let Some(event) = current.take() {
                    events.push(event);
                }
            }
            _ => {
                let Some(event) = current.as_mut() else {
                    continue;
                };
                let Some((name, value, params)) = parse_ical_property(&line) else {
                    continue;
                };
                match name.as_str() {
                    "UID" => event.uid = Some(unescape_ical_text(&value)),
                    "SUMMARY" => event.summary = Some(unescape_ical_text(&value)),
                    "DESCRIPTION" => event.description = Some(unescape_ical_text(&value)),
                    "LOCATION" => event.location = Some(unescape_ical_text(&value)),
                    "STATUS" => event.status = Some(value),
                    "DTSTART" => {
                        let (normalized, all_day) = normalize_ical_datetime(
                            &value,
                            params
                                .iter()
                                .find_map(|(key, value)| (key == "TZID").then_some(value.as_str())),
                            params.iter().find_map(|(key, value)| {
                                (key == "VALUE").then_some(value.as_str())
                            }),
                        );
                        event.start = Some(normalized);
                        event.all_day = all_day;
                    }
                    "DTEND" => {
                        let (normalized, _) = normalize_ical_datetime(
                            &value,
                            params
                                .iter()
                                .find_map(|(key, value)| (key == "TZID").then_some(value.as_str())),
                            params.iter().find_map(|(key, value)| {
                                (key == "VALUE").then_some(value.as_str())
                            }),
                        );
                        event.end = Some(normalized);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(events)
}

fn unfold_ical_lines(calendar_data: &str) -> Vec<String> {
    let mut unfolded: Vec<String> = Vec::new();
    for raw_line in calendar_data.replace("\r\n", "\n").split('\n') {
        if let Some(rest) = raw_line.strip_prefix(' ')
            && let Some(last) = unfolded.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix('\t')
            && let Some(last) = unfolded.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        unfolded.push(raw_line.to_string());
    }
    unfolded
}

fn parse_ical_property(line: &str) -> Option<IcalProperty> {
    let (left, value) = line.split_once(':')?;
    let mut segments = left.split(';');
    let name = segments.next()?.to_uppercase();
    let params = segments
        .filter_map(|segment| {
            let (key, value) = segment.split_once('=')?;
            Some((key.to_uppercase(), value.to_string()))
        })
        .collect::<Vec<_>>();
    Some((name, value.to_string(), params))
}

fn normalize_ical_datetime(
    raw: &str,
    tzid: Option<&str>,
    value_type: Option<&str>,
) -> (String, bool) {
    if value_type == Some("DATE") || raw.len() == 8 {
        if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y%m%d") {
            return (date.format("%Y-%m-%d").to_string(), true);
        }
        return (raw.to_string(), true);
    }

    if let Ok(datetime) = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ") {
        return (
            datetime
                .and_utc()
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            false,
        );
    }

    if let Ok(datetime) = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S") {
        let base = datetime.format("%Y-%m-%dT%H:%M:%S").to_string();
        return match tzid {
            Some(tzid) => (format!("{base} [{tzid}]"), false),
            None => (base, false),
        };
    }

    match tzid {
        Some(tzid) => (format!("{raw} [{tzid}]"), false),
        None => (raw.to_string(), false),
    }
}

fn unescape_ical_text(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

fn escape_ical_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_time_boundary(value: &str, flag_name: &str) -> Result<DateTime<Utc>> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let datetime = date.and_hms_opt(0, 0, 0).ok_or_else(|| {
            YacliError::Validation(format!("{flag_name} contains an invalid date"))
        })?;
        return Ok(datetime.and_utc());
    }

    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| {
            YacliError::Validation(format!(
                "{flag_name} must use YYYY-MM-DD or RFC3339, got `{value}`"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        CalendarInvite, build_calendar_event_ical, build_calendar_query_xml,
        build_calendar_uid_query_xml, calendar_create_request_from_invites,
        normalize_ical_datetime, parse_calendar_invites, parse_create_event_window,
        parse_dav_responses, parse_event_window, parse_ical_events,
    };

    #[test]
    fn parse_event_window_accepts_date_and_rfc3339_inputs() {
        let window = parse_event_window(Some("2026-03-12"), Some("2026-03-13T12:00:00Z"), 20)
            .expect("window");

        assert_eq!(window.from, "2026-03-12T00:00:00Z");
        assert_eq!(window.to, "2026-03-13T12:00:00Z");
        assert_eq!(window.limit, 20);
    }

    #[test]
    fn parse_dav_responses_extracts_calendar_home_and_data() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/first.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"abc"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
BEGIN:VEVENT
UID:one
SUMMARY:Test
DTSTART:20260312T090000Z
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        let responses = parse_dav_responses(xml).expect("responses");
        assert_eq!(responses.len(), 2);
        assert_eq!(
            responses[0].calendar_home_set.as_deref(),
            Some("/calendars/me@yandex.ru/")
        );
        assert_eq!(responses[1].getetag.as_deref(), Some("\"abc\""));
        assert!(
            responses[1]
                .calendar_data
                .as_deref()
                .unwrap_or_default()
                .contains("BEGIN:VEVENT")
        );
    }

    #[test]
    fn parse_ical_events_extracts_fields_and_unfolds_lines() {
        let events = parse_ical_events(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt-1\r\nSUMMARY:Команда\\, встреча\r\nDESCRIPTION:Первая строка\\n вторая\r\nDTSTART;TZID=Europe/Moscow:20260312T090000\r\nDTEND;TZID=Europe/Moscow:20260312T100000\r\nLOCATION:Мадрид\r\nEND:VEVENT\r\nEND:VCALENDAR",
        )
        .expect("events");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid.as_deref(), Some("evt-1"));
        assert_eq!(events[0].summary.as_deref(), Some("Команда, встреча"));
        assert_eq!(events[0].location.as_deref(), Some("Мадрид"));
        assert_eq!(
            events[0].start.as_deref(),
            Some("2026-03-12T09:00:00 [Europe/Moscow]")
        );
    }

    #[test]
    fn parse_calendar_invites_maps_raw_events_to_public_invites() {
        let invites = parse_calendar_invites(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt-1\r\nSUMMARY:Команда\\, встреча\r\nDTSTART:20260312T090000Z\r\nDTEND:20260312T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR",
        )
        .expect("invites");

        assert_eq!(invites.len(), 1);
        assert_eq!(invites[0].uid.as_deref(), Some("evt-1"));
        assert_eq!(invites[0].summary.as_deref(), Some("Команда, встреча"));
        assert_eq!(invites[0].start.as_deref(), Some("2026-03-12T09:00:00Z"));
        assert_eq!(invites[0].end.as_deref(), Some("2026-03-12T10:00:00Z"));
        assert!(!invites[0].all_day);
    }

    #[test]
    fn calendar_create_request_from_invites_selects_requested_event() {
        let invites = parse_calendar_invites(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt-1\r\nSUMMARY:Первый\r\nDTSTART:20260312T090000Z\r\nDTEND:20260312T100000Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:evt-2\r\nSUMMARY:Второй\r\nDTSTART:20260313T110000Z\r\nDTEND:20260313T120000Z\r\nLOCATION:Переговорка\r\nEND:VEVENT\r\nEND:VCALENDAR",
        )
        .expect("invites");

        let (request, selected) = calendar_create_request_from_invites(
            "default",
            &invites,
            2,
            "mail invite create-event",
        )
        .expect("request");

        assert_eq!(request.calendar, "default");
        assert_eq!(request.summary, "Второй");
        assert_eq!(request.start, "2026-03-13T11:00:00Z");
        assert_eq!(request.end, "2026-03-13T12:00:00Z");
        assert_eq!(request.location.as_deref(), Some("Переговорка"));
        assert_eq!(selected.uid.as_deref(), Some("evt-2"));
    }

    #[test]
    fn calendar_create_request_from_invites_requires_event_fields() {
        let invites = vec![CalendarInvite {
            uid: Some("evt-1".to_string()),
            summary: None,
            start: Some("2026-03-13T11:00:00Z".to_string()),
            end: Some("2026-03-13T12:00:00Z".to_string()),
            description: None,
            location: None,
            status: None,
            all_day: false,
        }];

        let error = calendar_create_request_from_invites(
            "default",
            &invites,
            1,
            "mail invite create-event",
        )
        .expect_err("validation error");
        assert!(
            error
                .to_string()
                .contains("mail invite create-event selected VEVENT is missing SUMMARY")
        );
    }

    #[test]
    fn normalize_ical_datetime_formats_utc_and_all_day_values() {
        let (utc_value, utc_all_day) = normalize_ical_datetime("20260312T090000Z", None, None);
        assert_eq!(utc_value, "2026-03-12T09:00:00Z");
        assert!(!utc_all_day);

        let (date_value, date_all_day) = normalize_ical_datetime("20260312", None, Some("DATE"));
        assert_eq!(date_value, "2026-03-12");
        assert!(date_all_day);
    }

    #[test]
    fn build_calendar_query_xml_contains_requested_window() {
        let window = parse_event_window(
            Some("2026-03-12T00:00:00Z"),
            Some("2026-03-19T00:00:00Z"),
            10,
        )
        .expect("window");
        let query = build_calendar_query_xml(&super::CalendarEventsRequest {
            calendar: "default".to_string(),
            from: chrono::DateTime::parse_from_rfc3339(&window.from)
                .expect("from")
                .with_timezone(&chrono::Utc),
            to: chrono::DateTime::parse_from_rfc3339(&window.to)
                .expect("to")
                .with_timezone(&chrono::Utc),
            limit: window.limit,
        });

        assert!(query.contains("start=\"20260312T000000Z\""));
        assert!(query.contains("end=\"20260319T000000Z\""));
    }

    #[test]
    fn parse_create_event_window_supports_timed_and_all_day_inputs() {
        let timed =
            parse_create_event_window("2026-03-12T09:00:00+01:00", "2026-03-12T10:00:00+01:00")
                .expect("timed window");
        assert_eq!(timed.normalized_start, "2026-03-12T08:00:00Z");
        assert_eq!(timed.normalized_end, "2026-03-12T09:00:00Z");
        assert!(!timed.all_day);

        let all_day =
            parse_create_event_window("2026-03-12", "2026-03-13").expect("all-day window");
        assert_eq!(all_day.normalized_start, "2026-03-12");
        assert_eq!(all_day.normalized_end, "2026-03-13");
        assert!(all_day.all_day);
    }

    #[test]
    fn build_calendar_event_ical_serializes_fields() {
        let window = parse_create_event_window("2026-03-12T09:00:00Z", "2026-03-12T10:00:00Z")
            .expect("window");
        let ical = build_calendar_event_ical(
            "evt-1",
            "Команда, встреча",
            Some("Первая строка\nвторая"),
            Some("Мадрид;офис"),
            &window,
        );

        assert!(ical.contains("UID:evt-1"));
        assert!(ical.contains("SUMMARY:Команда\\, встреча"));
        assert!(ical.contains("DESCRIPTION:Первая строка\\nвторая"));
        assert!(ical.contains("LOCATION:Мадрид\\;офис"));
        assert!(ical.contains("DTSTART:20260312T090000Z"));
        assert!(ical.contains("DTEND:20260312T100000Z"));
    }

    #[test]
    fn build_calendar_uid_query_xml_escapes_uid_text() {
        let query = build_calendar_uid_query_xml("evt<&>\"'");
        assert!(query.contains("evt&lt;&amp;&gt;&quot;&apos;"));
        assert!(query.contains("prop-filter name=\"UID\""));
    }
}
