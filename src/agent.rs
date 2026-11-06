//! Local API that lets an agent on this machine read chats and reply.
//!
//! The backend worker owns the only connection to the encrypted archive, so a
//! request crosses the command channel and the answer comes back on a oneshot.
//! Transports build a [`Request`] and render the [`Reply`]; none of them
//! touches the archive, the protocol library, or the interface.

pub mod http;
pub mod mcp;
pub mod token;
pub mod wire;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::oneshot;

/// Largest page a request may ask for.
pub const MAX_PAGE: usize = 200;

/// Page size when the caller names none.
pub const DEFAULT_PAGE: usize = 50;

/// Agent-initiated sends allowed per [`SEND_WINDOW`].
pub const SENDS_PER_WINDOW: usize = 20;

/// Rolling window the send cap counts over.
pub const SEND_WINDOW: Duration = Duration::from_secs(60);

/// Page boundary for [`Request::GetMessages`]: read what is older than this.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// Unix seconds of the oldest message already seen.
    pub timestamp: i64,
    pub id: String,
}

/// One thing an agent asks for. Every variant is answered by the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Status,
    ListChats {
        limit: usize,
        query: Option<String>,
        unread_only: bool,
        include_archived: bool,
    },
    GetChat {
        chat: String,
    },
    GetMessages {
        chat: String,
        limit: usize,
        before: Option<Cursor>,
    },
    SearchMessages {
        query: String,
        limit: usize,
    },
    ListContacts {
        query: Option<String>,
        limit: usize,
    },
    SendText {
        chat: String,
        text: String,
        quoting: Option<String>,
        mentions: Vec<String>,
    },
    React {
        chat: String,
        message: String,
        emoji: String,
    },
    MarkRead {
        chat: String,
    },
}

/// What the worker sends back. Shapes live in [`wire`].
pub type Reply = Result<serde_json::Value, Failure>;

/// A request and the channel its answer goes down.
pub struct Call {
    pub request: Request,
    pub reply: oneshot::Sender<Reply>,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only the method name. Chat ids and message text are personal data.
        formatter
            .debug_struct("Call")
            .field("method", &self.request.method())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No such chat, message, or contact in the archive.
    NotFound(String),
    /// Unknown method, bad arguments, or a value out of range.
    Invalid(String),
    /// Writing is switched off, or the send cap is spent.
    Refused(String),
    /// No phone link, so nothing can leave the machine.
    Offline,
    /// The archive or the protocol library failed.
    Internal(String),
}

impl Failure {
    /// Stable machine-readable name. Callers branch on this, not on the text.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::Invalid(_) => "invalid_request",
            Self::Refused(_) => "refused",
            Self::Offline => "offline",
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::NotFound(_) => 404,
            Self::Invalid(_) => 400,
            Self::Refused(_) => 403,
            Self::Offline => 503,
            Self::Internal(_) => 500,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotFound(what) => format!("no {what} by that id"),
            Self::Invalid(why) | Self::Refused(why) | Self::Internal(why) => why.clone(),
            Self::Offline => "Merlin is not linked to a phone right now".to_owned(),
        }
    }

    /// Error body shared by every transport.
    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({ "error": { "code": self.code(), "message": self.message() } })
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

/// Clamps a caller's page size into range.
fn page(asked: Option<usize>) -> usize {
    asked.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

fn required(value: Option<String>, field: &str) -> Result<String, Failure> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(Failure::Invalid(format!("{field} is required"))),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ListChatsArgs {
    limit: Option<usize>,
    query: Option<String>,
    unread_only: bool,
    include_archived: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ChatArgs {
    chat: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GetMessagesArgs {
    chat: Option<String>,
    limit: Option<usize>,
    before: Option<Cursor>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SearchArgs {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ListContactsArgs {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SendTextArgs {
    chat: Option<String>,
    text: Option<String>,
    quoting: Option<String>,
    mentions: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ReactArgs {
    chat: Option<String>,
    message: Option<String>,
    emoji: Option<String>,
}

impl Request {
    /// Method name used by every transport and by the tool catalogue.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::ListChats { .. } => "list_chats",
            Self::GetChat { .. } => "get_chat",
            Self::GetMessages { .. } => "get_messages",
            Self::SearchMessages { .. } => "search_messages",
            Self::ListContacts { .. } => "list_contacts",
            Self::SendText { .. } => "send_text",
            Self::React { .. } => "react",
            Self::MarkRead { .. } => "mark_read",
        }
    }

    /// Whether answering this reaches WhatsApp rather than only the archive.
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Self::SendText { .. } | Self::React { .. } | Self::MarkRead { .. }
        )
    }

    /// Builds a request from a method name and its arguments.
    ///
    /// Unknown fields are rejected rather than ignored, so an agent that
    /// misspells one is told instead of quietly getting the default.
    pub fn from_call(method: &str, arguments: serde_json::Value) -> Result<Self, Failure> {
        let arguments = match arguments {
            serde_json::Value::Null => serde_json::Value::Object(Default::default()),
            arguments => arguments,
        };
        fn parse<T: serde::de::DeserializeOwned>(
            arguments: serde_json::Value,
        ) -> Result<T, Failure> {
            serde_json::from_value(arguments).map_err(|error| Failure::Invalid(error.to_string()))
        }
        match method {
            "status" => Ok(Self::Status),
            "list_chats" => {
                let args: ListChatsArgs = parse(arguments)?;
                Ok(Self::ListChats {
                    limit: page(args.limit),
                    query: args.query.filter(|query| !query.trim().is_empty()),
                    unread_only: args.unread_only,
                    include_archived: args.include_archived,
                })
            }
            "get_chat" => {
                let args: ChatArgs = parse(arguments)?;
                Ok(Self::GetChat {
                    chat: required(args.chat, "chat")?,
                })
            }
            "get_messages" => {
                let args: GetMessagesArgs = parse(arguments)?;
                Ok(Self::GetMessages {
                    chat: required(args.chat, "chat")?,
                    limit: page(args.limit),
                    before: args.before,
                })
            }
            "search_messages" => {
                let args: SearchArgs = parse(arguments)?;
                Ok(Self::SearchMessages {
                    query: required(args.query, "query")?,
                    limit: page(args.limit),
                })
            }
            "list_contacts" => {
                let args: ListContactsArgs = parse(arguments)?;
                Ok(Self::ListContacts {
                    query: args.query.filter(|query| !query.trim().is_empty()),
                    limit: page(args.limit),
                })
            }
            "send_text" => {
                let args: SendTextArgs = parse(arguments)?;
                Ok(Self::SendText {
                    chat: required(args.chat, "chat")?,
                    text: required(args.text, "text")?,
                    quoting: args.quoting.filter(|id| !id.is_empty()),
                    mentions: args.mentions,
                })
            }
            "react" => {
                let args: ReactArgs = parse(arguments)?;
                Ok(Self::React {
                    chat: required(args.chat, "chat")?,
                    message: required(args.message, "message")?,
                    // An empty emoji removes a reaction, so it is allowed.
                    emoji: args.emoji.unwrap_or_default(),
                })
            }
            "mark_read" => {
                let args: ChatArgs = parse(arguments)?;
                Ok(Self::MarkRead {
                    chat: required(args.chat, "chat")?,
                })
            }
            other => Err(Failure::Invalid(format!("no method named {other}"))),
        }
    }
}

/// How the local agent API starts, if it starts at all.
#[derive(Debug)]
pub struct Setup {
    pub policy: Arc<Policy>,
    /// Loopback port, or `None` to serve nothing.
    pub port: Option<u16>,
    pub token_file: std::path::PathBuf,
}

impl Setup {
    /// Builds the setup the user's settings ask for.
    pub fn from_settings(
        settings: &crate::settings::Settings,
        dirs: &crate::paths::AppDirs,
    ) -> Self {
        Self {
            policy: Policy::new(
                settings.agent_api,
                settings.agent_can_send,
                settings.send_read_receipts,
            ),
            port: settings
                .agent_api
                .then(|| crate::profile::Profile::current().agent_port()),
            token_file: dirs.agent_token_file(),
        }
    }

    /// Nothing served and nothing writable: demos, tests, and detached runs.
    pub fn off() -> Self {
        Self {
            policy: Policy::off(),
            port: None,
            token_file: std::path::PathBuf::new(),
        }
    }
}

/// What the worker may do for an agent, mirrored from the user's settings.
///
/// The interface owns the settings file and writes here; the worker reads.
/// Keeping the check in the worker means every transport is gated by it, and a
/// flag flipped in Settings takes effect on the next request with no restart.
#[derive(Debug)]
pub struct Policy {
    enabled: AtomicBool,
    send: AtomicBool,
    read_receipts: AtomicBool,
}

impl Policy {
    pub fn new(enabled: bool, send: bool, read_receipts: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            send: AtomicBool::new(send),
            read_receipts: AtomicBool::new(read_receipts),
        })
    }

    /// Answers nothing, which is what a detached or demo backend gets.
    pub fn off() -> Arc<Self> {
        Self::new(false, false, false)
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn may_send(&self) -> bool {
        self.enabled() && self.send.load(Ordering::Relaxed)
    }

    /// Whether a read mark also tells the sender, per the privacy setting.
    pub fn read_receipts(&self) -> bool {
        self.read_receipts.load(Ordering::Relaxed)
    }

    pub fn set(&self, enabled: bool, send: bool, read_receipts: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.send.store(send, Ordering::Relaxed);
        self.read_receipts.store(read_receipts, Ordering::Relaxed);
    }

    /// Refuses what the user has not allowed.
    ///
    /// Switching the API off stops answers at once, even though the listener
    /// only goes away when Merlin next starts.
    pub fn gate(&self, request: &Request) -> Result<(), Failure> {
        if !self.enabled() {
            return Err(Failure::Refused(
                "the local agent API is switched off in Settings".to_owned(),
            ));
        }
        if request.writes() && !self.may_send() {
            return Err(Failure::Refused(
                "the local agent API is set to read only; turn on agent sending in Settings"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

/// Rolling cap on agent-initiated sends, so a looping agent cannot flood a chat.
#[derive(Debug, Default)]
pub struct SendBudget {
    spent: VecDeque<Instant>,
}

impl SendBudget {
    /// Spends one send, or refuses when the window is full.
    pub fn take(&mut self) -> Result<(), Failure> {
        self.take_at(Instant::now())
    }

    fn take_at(&mut self, now: Instant) -> Result<(), Failure> {
        while let Some(&oldest) = self.spent.front() {
            if now.duration_since(oldest) >= SEND_WINDOW {
                self.spent.pop_front();
            } else {
                break;
            }
        }
        if self.spent.len() >= SENDS_PER_WINDOW {
            return Err(Failure::Refused(format!(
                "the agent send cap of {SENDS_PER_WINDOW} a minute is spent; try again shortly"
            )));
        }
        self.spent.push_back(now);
        Ok(())
    }

    /// Sends still available in the current window.
    pub fn left(&self) -> usize {
        SENDS_PER_WINDOW.saturating_sub(self.spent.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_size_stays_in_range() {
        assert_eq!(page(None), DEFAULT_PAGE);
        assert_eq!(page(Some(0)), 1);
        assert_eq!(page(Some(10)), 10);
        assert_eq!(page(Some(usize::MAX)), MAX_PAGE);
    }

    #[test]
    fn a_misspelled_argument_is_reported_rather_than_ignored() {
        let arguments = serde_json::json!({ "chat": "1@s.whatsapp.net", "txt": "hello" });
        let failure = Request::from_call("send_text", arguments).expect_err("a rejection");
        assert_eq!(failure.code(), "invalid_request");
        assert!(failure.message().contains("txt"), "{failure}");
    }

    #[test]
    fn a_request_without_its_subject_is_refused() {
        for (method, field) in [
            ("get_chat", "chat"),
            ("get_messages", "chat"),
            ("search_messages", "query"),
            ("mark_read", "chat"),
        ] {
            let failure =
                Request::from_call(method, serde_json::json!({})).expect_err("a rejection");
            assert_eq!(failure.message(), format!("{field} is required"));
        }
        // Whitespace is not a chat id either.
        let failure = Request::from_call("get_chat", serde_json::json!({ "chat": "  " }))
            .expect_err("a rejection");
        assert_eq!(failure.message(), "chat is required");
    }

    #[test]
    fn an_unknown_method_names_itself_in_the_rejection() {
        let failure = Request::from_call("delete_everything", serde_json::Value::Null)
            .expect_err("a rejection");
        assert_eq!(failure.status(), 400);
        assert!(failure.message().contains("delete_everything"), "{failure}");
    }

    #[test]
    fn status_takes_no_arguments_and_needs_none() {
        assert_eq!(
            Request::from_call("status", serde_json::Value::Null).expect("a request"),
            Request::Status
        );
    }

    /// An empty emoji is how a reaction is withdrawn, so it must survive.
    #[test]
    fn an_empty_reaction_is_a_withdrawal_not_a_mistake() {
        let arguments = serde_json::json!({ "chat": "1@s.whatsapp.net", "message": "AB" });
        let request = Request::from_call("react", arguments).expect("a request");
        assert_eq!(
            request,
            Request::React {
                chat: "1@s.whatsapp.net".to_owned(),
                message: "AB".to_owned(),
                emoji: String::new(),
            }
        );
    }

    #[test]
    fn only_the_methods_that_reach_whatsapp_count_as_writes() {
        let reads = [
            ("status", serde_json::json!({})),
            ("list_chats", serde_json::json!({})),
            ("get_chat", serde_json::json!({ "chat": "c" })),
            ("get_messages", serde_json::json!({ "chat": "c" })),
            ("search_messages", serde_json::json!({ "query": "x" })),
            ("list_contacts", serde_json::json!({})),
        ];
        for (method, arguments) in reads {
            let request = Request::from_call(method, arguments).expect("a request");
            assert!(!request.writes(), "{method} should read only");
        }
        let writes = [
            Request::from_call(
                "send_text",
                serde_json::json!({ "chat": "c", "text": "hi" }),
            ),
            Request::from_call("mark_read", serde_json::json!({ "chat": "c" })),
            Request::from_call(
                "react",
                serde_json::json!({ "chat": "c", "message": "m", "emoji": "👍" }),
            ),
        ];
        for request in writes {
            let request = request.expect("a request");
            assert!(request.writes(), "{} should write", request.method());
        }
    }

    #[test]
    fn the_send_cap_refuses_a_flood_and_recovers_after_the_window() {
        let mut budget = SendBudget::default();
        let start = Instant::now();
        for _ in 0..SENDS_PER_WINDOW {
            budget.take_at(start).expect("within the cap");
        }
        assert_eq!(budget.left(), 0);
        let refusal = budget.take_at(start).expect_err("over the cap");
        assert_eq!(refusal.code(), "refused");
        assert_eq!(refusal.status(), 403);

        // The window is rolling, not a reset at a fixed minute.
        budget
            .take_at(start + SEND_WINDOW)
            .expect("the window has moved on");
        assert_eq!(budget.left(), SENDS_PER_WINDOW - 1);
    }

    /// A chat id in a log is personal data, and `Debug` is what reaches logs.
    #[test]
    fn a_logged_call_carries_the_method_and_nothing_else() {
        let (reply, _receiver) = oneshot::channel();
        let call = Call {
            request: Request::SendText {
                chat: "573001234567@s.whatsapp.net".to_owned(),
                text: "meet me at the usual place".to_owned(),
                quoting: None,
                mentions: Vec::new(),
            },
            reply,
        };
        let logged = format!("{call:?}");
        assert!(logged.contains("send_text"), "{logged}");
        assert!(!logged.contains("573001234567"), "{logged}");
        assert!(!logged.contains("usual place"), "{logged}");
    }

    #[test]
    fn a_read_only_policy_refuses_writes_and_allows_reads() {
        let policy = Policy::new(true, false, true);
        let send = Request::from_call("send_text", serde_json::json!({ "chat": "c", "text": "x" }))
            .expect("a request");
        let list = Request::from_call("list_chats", serde_json::json!({})).expect("a request");
        let refusal = policy.gate(&send).expect_err("a refusal");
        assert_eq!(refusal.code(), "refused");
        assert!(refusal.message().contains("read only"), "{refusal}");
        policy.gate(&list).expect("reads are always allowed");

        // Flipping the setting takes effect without a restart.
        policy.set(true, true, true);
        policy.gate(&send).expect("sending is now permitted");
        assert!(policy.read_receipts());
    }

    /// Switching the API off must stop reads too, while the listener is still
    /// bound until Merlin next starts.
    #[test]
    fn a_switched_off_policy_refuses_everything_including_reads() {
        let policy = Policy::off();
        for (method, arguments) in [
            ("status", serde_json::json!({})),
            ("list_chats", serde_json::json!({})),
            ("send_text", serde_json::json!({ "chat": "c", "text": "x" })),
        ] {
            let request = Request::from_call(method, arguments).expect("a request");
            let refusal = policy.gate(&request).expect_err("a refusal");
            assert!(refusal.message().contains("switched off"), "{method}");
        }
        // Sending stays refused while the API itself is off.
        policy.set(false, true, true);
        assert!(!policy.may_send());
    }
}
