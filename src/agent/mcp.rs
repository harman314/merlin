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
pub const PROTOCOL_VERSION: &str = "2025-06-18";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug, Deserialize)]
struct Message {
    /// Absent on a notification, which takes no reply.
    #[serde(default)]
    id: Option<serde_json::Value>,
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

fn tool(name: &str, description: &str, schema: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "name": name, "description": description, "inputSchema": schema })
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
    let message: Message = match serde_json::from_slice(body) {
        Ok(message) => message,
        Err(error) => {
            // Without an id there is nobody to answer, so report against null.
            return Some(error_for(
                serde_json::Value::Null,
                PARSE_ERROR,
                &format!("could not read the message: {error}"),
            ));
        }
    };
    let Some(id) = message.id.clone() else {
        // `notifications/initialized` and friends: nothing to send back.
        return None;
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
                        INVALID_REQUEST,
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
