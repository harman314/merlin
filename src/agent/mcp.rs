//! MCP over HTTP, so an MCP client reaches the same handlers as the plain
//! JSON routes.
//!
//! The server is stateless: it keeps no session, streams nothing, and answers
//! each `POST` with one JSON-RPC response. That is what the transport permits
//! when there is nothing to stream, and it keeps the surface small.

use serde::Deserialize;

use super::Request;
use super::http::Service;

/// Revision of the protocol this speaks.
/// The one revision this server speaks.
///
/// `structuredContent`, which every answer here carries, arrived in this
/// revision. A stateless server cannot remember what it negotiated, so it
/// cannot drop that field for an older client, and claiming a revision it
/// would then violate is worse than naming one. A client that asked for
/// something else gets this back and decides whether to carry on, which is
/// what the transport asks of a server with no version in common.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

#[derive(Debug, Deserialize)]
struct Message {
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
    /// The protocol reserves `_meta` on every request, and clients put a
    /// progress token in it. Nothing here acts on it, but the envelope has to
    /// accept it or the call never reaches the tool. Unknown fields stay
    /// refused, and tool arguments reject their own unknown names separately.
    #[serde(default)]
    _meta: serde_json::Value,
}

fn object(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn limit(most: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "integer",
        "minimum": 1,
        "maximum": most,
        "description": "How many to return. Defaults to 50.",
    })
}

fn chat_id() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "description": "Conversation id from list_chats, such as 15551234567@s.whatsapp.net or 1234-5678@g.us.",
    })
}

/// A tool that only reads the archive on this machine.
fn reads() -> serde_json::Value {
    // `destructiveHint` and `idempotentHint` only carry meaning for a tool
    // that writes, so a read-only tool leaves them out.
    serde_json::json!({ "readOnlyHint": true, "openWorldHint": false })
}

/// A tool that reaches WhatsApp. `destructive` marks one a client should
/// confirm first, because other people see what it did.
fn writes(destructive: bool) -> serde_json::Value {
    serde_json::json!({
        "readOnlyHint": false,
        "destructiveHint": destructive,
        "idempotentHint": !destructive,
        "openWorldHint": true,
    })
}

/// Display title and annotations per tool.
///
/// A name with no entry is advertised as writing and destructive, so a tool
/// added without one is over-restricted rather than passed off as read-only. A
/// test catches the missing title.
fn facts(name: &str) -> (&'static str, serde_json::Value) {
    match name {
        "status" => ("Status", reads()),
        "list_chats" => ("List chats", reads()),
        "get_chat" => ("Chat details", reads()),
        "get_messages" => ("Read messages", reads()),
        "search_messages" => ("Search messages", reads()),
        "list_contacts" => ("List contacts", reads()),
        "send_text" => ("Send a message", writes(true)),
        "react" => ("React to a message", writes(false)),
        "mark_read" => ("Mark a chat read", writes(false)),
        _ => ("", writes(true)),
    }
}

/// Builds one catalogue entry. The annotations let a client treat the three
/// writing tools differently without reading the prose. They are hints only:
/// what an agent may actually do is decided by `agent::Policy::gate`.
fn tool(name: &str, description: &str, schema: serde_json::Value) -> serde_json::Value {
    let (title, annotations) = facts(name);
    serde_json::json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": annotations,
    })
}

/// The catalogue an MCP client reads before calling anything.
pub fn tools() -> Vec<serde_json::Value> {
    let most = super::MAX_PAGE;
    vec![
        tool(
            "status",
            "Reports whether Merlin is linked to the phone, which account it is \
             linked as, and whether this agent is allowed to send. Call it first \
             when a result is surprising: an empty chat list usually means the \
             phone is not linked rather than that there are no chats.",
            object(serde_json::json!({}), &[]),
        ),
        tool(
            "list_chats",
            "Lists conversations, pinned first and then by most recent message. \
             Returns the ids every other tool takes.",
            object(
                serde_json::json!({
                    "limit": limit(most),
                    "query": {
                        "type": "string",
                        "description": "Match against the conversation name or phone number. Accents and case are ignored.",
                    },
                    "unread_only": {
                        "type": "boolean",
                        "description": "Only conversations with unread messages.",
                    },
                    "include_archived": {
                        "type": "boolean",
                        "description": "Include archived conversations, which are left out by default.",
                    },
                }),
                &[],
            ),
        ),
        tool(
            "get_chat",
            "One conversation with its flags, and for a group its members and \
             their names.",
            object(serde_json::json!({ "chat": chat_id() }), &["chat"]),
        ),
        tool(
            "get_messages",
            "Reads a page of messages from one conversation, oldest first. When \
             the answer carries next_before, pass it back as `before` to read \
             further into the past.",
            object(
                serde_json::json!({
                    "chat": chat_id(),
                    "limit": limit(most),
                    "before": {
                        "type": "object",
                        "description": "The next_before value from an earlier page.",
                        "properties": {
                            "timestamp": { "type": "integer" },
                            "id": { "type": "string" },
                        },
                        "required": ["timestamp", "id"],
                        "additionalProperties": false,
                    },
                }),
                &["chat"],
            ),
        ),
        tool(
            "search_messages",
            "Finds messages across every conversation whose text, caption, file \
             name, poll question or place name matches. Each result names the \
             conversation it came from.",
            object(
                serde_json::json!({
                    "query": { "type": "string", "description": "Text to look for." },
                    "limit": limit(most),
                }),
                &["query"],
            ),
        ),
        tool(
            "list_contacts",
            "Address-book and profile names Merlin knows, with the ids they map to.",
            object(
                serde_json::json!({
                    "query": { "type": "string", "description": "Match against the name or the id." },
                    "limit": limit(most),
                }),
                &[],
            ),
        ),
        tool(
            "send_text",
            "Sends a text message as the user, to a conversation that already \
             exists. Returns the new message id. Delivery happens afterwards, so \
             read the message back with get_messages to see its status.",
            object(
                serde_json::json!({
                    "chat": chat_id(),
                    "text": { "type": "string", "description": "What to send." },
                    "quoting": {
                        "type": "string",
                        "description": "Message id to reply to, from get_messages.",
                    },
                    "mentions": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Ids mentioned in the text, which must also appear in it as @number.",
                    },
                }),
                &["chat", "text"],
            ),
        ),
        tool(
            "react",
            "Puts a reaction on a message, or takes one off when emoji is empty.",
            object(
                serde_json::json!({
                    "chat": chat_id(),
                    "message": { "type": "string", "description": "Message id from get_messages." },
                    "emoji": { "type": "string", "description": "One emoji, or empty to remove." },
                }),
                &["chat", "message"],
            ),
        ),
        tool(
            "mark_read",
            "Clears a conversation's unread count, and tells the sender when the \
             user's own read-receipt setting permits it.",
            object(serde_json::json!({ "chat": chat_id() }), &["chat"]),
        ),
    ]
}

/// Answers one JSON-RPC message. `None` is a notification, which takes no reply.
pub async fn handle(service: &Service, body: &[u8]) -> Option<serde_json::Value> {
    let envelope: serde_json::Value = match serde_json::from_slice(body) {
        Ok(envelope) => envelope,
        Err(error) => {
            // Without an id there is nobody to answer, so report against null.
            return Some(error_for(
                serde_json::Value::Null,
                PARSE_ERROR,
                &format!("could not read the message: {error}"),
            ));
        }
    };
    // Batching left the protocol in this revision. Deserializing a struct from
    // an array otherwise reports a type error against the first element, which
    // reads like a malformed single message.
    if envelope.is_array() {
        return Some(error_for(
            serde_json::Value::Null,
            INVALID_REQUEST,
            "this server takes one message per request; batching is not part of \
             the protocol from 2025-06-18",
        ));
    }
    // An absent id is a notification. A null id is neither, and answering it
    // as a notification hides a client bug behind an empty 202.
    let id = match envelope.get("id") {
        None => return None,
        Some(serde_json::Value::Null) => {
            return Some(error_for(
                serde_json::Value::Null,
                INVALID_REQUEST,
                "a request id must not be null",
            ));
        }
        Some(id) => id.clone(),
    };
    let message: Message = match serde_json::from_value(envelope) {
        Ok(message) => message,
        Err(error) => {
            return Some(error_for(
                id,
                INVALID_REQUEST,
                &format!("could not read the message: {error}"),
            ));
        }
    };
    match message.method.as_str() {
        "initialize" => Some(result_for(
            id,
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "merlin",
                    "title": crate::profile::Profile::current().display_name(),
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": "Merlin is this machine's WhatsApp client. \
                    Conversation and message ids come from list_chats and \
                    get_messages; pass them back unchanged. Sending is subject \
                    to a per-minute cap and to a setting the user controls.",
            }),
        )),
        "ping" => Some(result_for(id, serde_json::json!({}))),
        "tools/list" => Some(result_for(id, serde_json::json!({ "tools": tools() }))),
        "tools/call" => {
            let call: ToolCall = match serde_json::from_value(message.params) {
                Ok(call) => call,
                Err(error) => {
                    return Some(error_for(
                        id,
                        INVALID_PARAMS,
                        &format!("could not read the tool call: {error}"),
                    ));
                }
            };
            Some(result_for(id, call_tool(service, call).await))
        }
        other => Some(error_for(
            id,
            METHOD_NOT_FOUND,
            &format!("no method named {other}"),
        )),
    }
}

/// A tool that refuses is a result with `isError`, not a protocol error: the
/// client shows the text to the model so it can choose what to do instead.
async fn call_tool(service: &Service, call: ToolCall) -> serde_json::Value {
    let answer = match Request::from_call(&call.name, call.arguments) {
        Ok(request) => service.ask(request).await,
        Err(failure) => Err(failure),
    };
    match answer {
        Ok(value) => serde_json::json!({
            "content": [{ "type": "text", "text": text_of(&value) }],
            "structuredContent": value,
            "isError": false,
        }),
        Err(failure) => serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!("{}: {}", failure.code(), failure.message()),
            }],
            "isError": true,
        }),
    }
}

fn text_of(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn result_for(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_for(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> Service {
        let (calls, _receiver) = tokio::sync::mpsc::unbounded_channel();
        Service {
            calls,
            token: super::super::token::Token::generate().expect("a token"),
        }
    }

    #[test]
    fn every_tool_matches_a_method_the_worker_answers() {
        for entry in tools() {
            let name = entry["name"].as_str().expect("a name");
            // An empty argument set must either build a request or say what is
            // missing. A tool nobody can route is the failure to catch.
            match Request::from_call(name, serde_json::json!({})) {
                Ok(_) => {}
                Err(failure) => assert_ne!(
                    failure.message(),
                    format!("no method named {name}"),
                    "{name} is advertised but not routed"
                ),
            }
        }
    }

    #[test]
    fn every_tool_declares_an_object_schema_with_a_description() {
        for entry in tools() {
            let name = entry["name"].as_str().expect("a name");
            assert!(
                entry["description"]
                    .as_str()
                    .is_some_and(|text| text.len() > 30),
                "{name} needs a description an agent can act on"
            );
            assert_eq!(entry["inputSchema"]["type"], "object", "{name}");
            assert_eq!(
                entry["inputSchema"]["additionalProperties"], false,
                "{name} must reject arguments it does not know"
            );
        }
    }

    /// Both writing tools must be present, since replying is the point.
    #[test]
    fn the_catalogue_covers_reading_and_replying() {
        let names: Vec<_> = tools()
            .iter()
            .map(|entry| entry["name"].as_str().expect("a name").to_owned())
            .collect();
        for expected in [
            "status",
            "list_chats",
            "get_chat",
            "get_messages",
            "search_messages",
            "list_contacts",
            "send_text",
            "react",
            "mark_read",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "{expected} is missing"
            );
        }
    }

    #[tokio::test]
    async fn initialize_names_the_protocol_and_the_tool_capability() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .await
        .expect("a reply");
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(answer["result"]["capabilities"]["tools"].is_object());
        assert_eq!(answer["result"]["serverInfo"]["name"], "merlin");
    }

    #[tokio::test]
    async fn a_notification_gets_no_reply() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;
        assert!(answer.is_none());
    }

    #[tokio::test]
    async fn an_unknown_method_is_a_protocol_error() {
        let answer = handle(&service(), br#"{"jsonrpc":"2.0","id":7,"method":"wat"}"#)
            .await
            .expect("a reply");
        assert_eq!(answer["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(answer["id"], 7);
    }

    #[tokio::test]
    async fn unreadable_json_is_reported_against_a_null_id() {
        let answer = handle(&service(), b"{not json").await.expect("a reply");
        assert_eq!(answer["error"]["code"], PARSE_ERROR);
        assert!(answer["id"].is_null());
    }

    /// A tool that cannot run reports itself inside the result, so the model
    /// sees the reason rather than the client seeing a broken server.
    #[tokio::test]
    async fn a_refused_tool_call_comes_back_as_a_tool_error() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call",
                 "params":{"name":"send_text","arguments":{"chat":"c"}}}"#,
        )
        .await
        .expect("a reply");
        assert!(answer["error"].is_null(), "{answer}");
        assert_eq!(answer["result"]["isError"], true);
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("text is required"), "{text}");
    }

    /// The bug this suite missed on the first pass. The protocol reserves
    /// `_meta` on every request and Claude Code puts a progress token there,
    /// so a strict envelope refused every tool call from every real client
    /// while these tests, which sent no `_meta`, stayed green.
    #[tokio::test]
    async fn a_progress_token_in_meta_does_not_stop_the_call() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                 "name":"send_text","arguments":{"chat":"c","text":"hi"},
                 "_meta":{"progressToken":"abc"}}}"#,
        )
        .await
        .expect("a reply");
        // The worker stub is not wired up here, so reaching it is the proof:
        // the envelope parsed and the tool ran.
        assert!(answer["error"].is_null(), "{answer}");
        assert!(answer["result"]["isError"].is_boolean(), "{answer}");
    }

    /// Accepting `_meta` must not turn into accepting anything.
    #[tokio::test]
    async fn an_invented_envelope_field_is_still_refused() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":5,"method":"tools/call",
                 "params":{"name":"status","arguments":{},"sudo":true}}"#,
        )
        .await
        .expect("a reply");
        assert_eq!(answer["error"]["code"], INVALID_PARAMS);
        let message = answer["error"]["message"].as_str().expect("text");
        assert!(message.contains("sudo"), "{message}");
    }

    #[tokio::test]
    async fn a_batch_is_refused_in_terms_a_client_can_act_on() {
        let answer = handle(&service(), br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#)
            .await
            .expect("a reply");
        assert_eq!(answer["error"]["code"], INVALID_REQUEST);
        let message = answer["error"]["message"].as_str().expect("text");
        assert!(message.contains("batching"), "{message}");
    }

    /// An absent id is a notification; a null id is a malformed request. Both
    /// used to come back as an empty acknowledgement.
    #[tokio::test]
    async fn a_null_id_is_a_bad_request_rather_than_a_notification() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
        )
        .await
        .expect("a reply");
        assert_eq!(answer["error"]["code"], INVALID_REQUEST);
        assert!(
            handle(&service(), br#"{"jsonrpc":"2.0","method":"ping"}"#)
                .await
                .is_none()
        );
    }

    /// A client that asks for a revision this server does not speak gets one
    /// it does, and decides for itself whether to carry on.
    #[tokio::test]
    async fn an_unknown_protocol_revision_still_gets_a_supported_one_back() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":6,"method":"initialize",
                 "params":{"protocolVersion":"2024-11-05"}}"#,
        )
        .await
        .expect("a reply");
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    /// Reading and sending are two separate switches in Settings. A client
    /// cannot honour that distinction if it has to read the prose to find it.
    #[test]
    fn only_the_three_writing_tools_are_marked_as_writing() {
        let mut writing: Vec<String> = Vec::new();
        for entry in tools() {
            let name = entry["name"].as_str().expect("a name");
            let annotations = &entry["annotations"];
            assert!(
                entry["title"]
                    .as_str()
                    .is_some_and(|title| !title.is_empty()),
                "{name} has no display title, so `facts` has no entry for it"
            );
            match annotations["readOnlyHint"].as_bool() {
                Some(true) => assert_eq!(annotations["openWorldHint"], false, "{name}"),
                Some(false) => writing.push(name.to_owned()),
                None => panic!("{name} declares no readOnlyHint"),
            }
        }
        writing.sort();
        assert_eq!(writing, ["mark_read", "react", "send_text"]);
    }

    /// Only a sent message cannot be taken back.
    #[test]
    fn sending_is_the_one_tool_marked_destructive() {
        let destructive: Vec<String> = tools()
            .iter()
            .filter(|entry| entry["annotations"]["destructiveHint"] == true)
            .map(|entry| entry["name"].as_str().expect("a name").to_owned())
            .collect();
        assert_eq!(destructive, ["send_text"]);
    }

    #[tokio::test]
    async fn the_tool_list_is_served_without_touching_the_worker() {
        let answer = handle(
            &service(),
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        )
        .await
        .expect("a reply");
        let listed = answer["result"]["tools"].as_array().expect("an array");
        assert_eq!(listed.len(), tools().len());
    }
}
