//! Shapes the local API sends back.
//!
//! These are deliberately not the application's own types. The archive row
//! carries a JPEG thumbnail, download state, and raw protobuf fields that no
//! agent needs, and pinning the interface's structs to a public API would
//! freeze them. Each builder takes what the worker already has to hand.

use serde::Serialize;

use crate::model::{Chat, ChatKind, Contact, Content, Delivery, Message};

/// Link and permission state, so an agent can tell "no chats" from "not linked".
#[derive(Debug, Serialize)]
pub struct Status {
    /// Profile display name, which differs between the installed and dev apps.
    pub app: &'static str,
    pub version: &'static str,
    /// Link state: `connected`, `connecting`, `unlinked`, and so on.
    pub link: &'static str,
    pub connected: bool,
    /// Whether history sync is still filling the archive.
    pub syncing: bool,
    /// Our own canonical id, needed to read `from_me` and mentions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub me: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub me_name: Option<String>,
    pub chats: usize,
    /// Whether sending is permitted at all.
    pub can_send: bool,
    /// Sends left in the current rate window.
    pub sends_left: usize,
}

#[derive(Debug, Serialize)]
pub struct ChatBrief {
    pub id: String,
    pub name: String,
    /// `direct`, `group`, or `broadcast`.
    pub kind: &'static str,
    pub unread: u32,
    pub pinned: bool,
    pub archived: bool,
    pub muted: bool,
    /// Announcement group where we cannot post.
    pub read_only: bool,
    /// Unix seconds of the latest message.
    pub last_activity: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<LastBrief>,
}

#[derive(Debug, Serialize)]
pub struct LastBrief {
    pub from_me: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub summary: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MessageBrief {
    pub id: String,
    pub chat: String,
    /// Chat name, filled where the chat is not already obvious: search results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_name: Option<String>,
    pub from_me: bool,
    /// Canonical sender id. Our own for outgoing messages.
    pub sender: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    /// Unix seconds. Pair with `id` to page further back.
    pub timestamp: i64,
    pub at: String,
    /// `text`, `image`, `poll`, and so on.
    pub kind: &'static str,
    /// Body or caption, when the message has readable text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// One-line description, the same one the chat list shows.
    pub summary: String,
    pub status: &'static str,
    pub edited: bool,
    pub forwarded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<AttachmentBrief>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<PollBrief>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place: Option<PlaceBrief>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quoted: Option<QuotedBrief>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionBrief>,
    /// Canonical ids mentioned in the text.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AttachmentBrief {
    pub mime: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Voice note rather than an attached audio file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub voice_note: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gif: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub animated: bool,
    /// Local file, present only once the attachment has been downloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PollBrief {
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PlaceBrief {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QuotedBrief {
    pub id: String,
    pub sender: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub summary: String,
}

#[derive(Debug, Serialize)]
pub struct ReactionBrief {
    pub emoji: String,
    pub sender: String,
    pub from_me: bool,
}

#[derive(Debug, Serialize)]
pub struct ContactBrief {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_name: Option<String>,
}

/// What a send returns. The message is archived and queued, not yet acked, so
/// the agent gets the id and can read the status back later.
#[derive(Debug, Serialize)]
pub struct Accepted {
    pub chat: String,
    pub message: String,
    pub status: &'static str,
}

/// RFC 3339 in UTC, which is what a machine reader wants.
pub fn at(unix_seconds: i64) -> Option<String> {
    jiff::Timestamp::from_second(unix_seconds)
        .ok()
        .map(|stamp| stamp.to_string())
}

pub fn kind_of(kind: ChatKind) -> &'static str {
    match kind {
        ChatKind::Direct => "direct",
        ChatKind::Group => "group",
        ChatKind::Broadcast => "broadcast",
    }
}

pub fn delivery(status: Delivery) -> &'static str {
    match status {
        Delivery::None => "none",
        Delivery::Pending => "pending",
        Delivery::Sent => "sent",
        Delivery::Delivered => "delivered",
        Delivery::Read => "read",
        Delivery::Played => "played",
        Delivery::Failed => "failed",
    }
}

pub fn content_kind(content: &Content) -> &'static str {
    match content {
        Content::Text { .. } => "text",
        Content::Image { .. } => "image",
        Content::Video { gif: true, .. } => "gif",
        Content::Video { .. } => "video",
        Content::Audio { .. } => "audio",
        Content::Document { .. } => "document",
        Content::Sticker { .. } => "sticker",
        Content::Location { .. } => "location",
        Content::Contact { .. } => "contact",
        Content::Poll { .. } => "poll",
        Content::Revoked => "deleted",
        Content::Unsupported { .. } => "unsupported",
    }
}

/// The readable body an agent would quote or reply to, if there is one.
fn body(content: &Content) -> Option<String> {
    let text = match content {
        Content::Text { text, .. } => text.clone(),
        Content::Image { caption, .. }
        | Content::Video { caption, .. }
        | Content::Document { caption, .. } => caption.clone()?,
        Content::Contact { display_name, .. } => display_name.clone(),
        _ => return None,
    };
    Some(text).filter(|text| !text.is_empty())
}

fn attachment(content: &Content) -> Option<AttachmentBrief> {
    let brief = |media: &crate::model::Media| AttachmentBrief {
        mime: media.mime.clone(),
        size: media.size,
        file_name: None,
        seconds: None,
        width: media.width,
        height: media.height,
        voice_note: false,
        gif: false,
        animated: false,
        path: media.path.as_ref().map(|path| path.display().to_string()),
    };
    match content {
        Content::Image { media, .. } | Content::Sticker { media, .. } => {
            let mut built = brief(media);
            if let Content::Sticker { animated, .. } = content {
                built.animated = *animated;
            }
            Some(built)
        }
        Content::Video {
            media,
            seconds,
            gif,
            ..
        } => Some(AttachmentBrief {
            seconds: *seconds,
            gif: *gif,
            ..brief(media)
        }),
        Content::Audio {
            media,
            seconds,
            voice_note,
            ..
        } => Some(AttachmentBrief {
            seconds: *seconds,
            voice_note: *voice_note,
            ..brief(media)
        }),
        Content::Document {
            media, file_name, ..
        } => Some(AttachmentBrief {
            file_name: Some(file_name.clone()).filter(|name| !name.is_empty()),
            ..brief(media)
        }),
        _ => None,
    }
}

pub fn chat(chat: &Chat) -> ChatBrief {
    ChatBrief {
        id: chat.id.clone(),
        name: chat.name.clone(),
        kind: kind_of(chat.kind),
        unread: chat.unread,
        pinned: chat.pinned,
        archived: chat.archived,
        muted: chat.muted_until.is_some(),
        read_only: chat.read_only,
        last_activity: chat.last_activity,
        last_activity_at: at(chat.last_activity),
        last: chat.last.as_ref().map(|last| LastBrief {
            from_me: last.from_me,
            sender_name: last.sender_name.clone(),
            summary: last.summary.clone(),
            status: delivery(last.status),
        }),
    }
}

/// `sender_name` is the worker's best name for the sender, which the archive
/// row only knows for group messages.
pub fn message(message: &Message, sender_name: Option<String>) -> MessageBrief {
    MessageBrief {
        id: message.id.clone(),
        chat: message.chat.clone(),
        chat_name: None,
        from_me: message.from_me,
        sender: message.sender.clone(),
        sender_name: sender_name.or_else(|| message.sender_name.clone()),
        timestamp: message.timestamp,
        at: at(message.timestamp).unwrap_or_default(),
        kind: content_kind(&message.content),
        text: body(&message.content),
        summary: message.content.summary(),
        status: delivery(message.status),
        edited: message.edited,
        forwarded: message.forwarded,
        attachment: attachment(&message.content),
        poll: match &message.content {
            Content::Poll {
                question, options, ..
            } => Some(PollBrief {
                question: question.clone(),
                options: options.clone(),
            }),
            _ => None,
        },
        place: match &message.content {
            Content::Location {
                latitude,
                longitude,
                name,
                address,
            } => Some(PlaceBrief {
                latitude: *latitude,
                longitude: *longitude,
                name: name.clone(),
                address: address.clone(),
            }),
            _ => None,
        },
        quoted: message.quoted.as_ref().map(|quoted| QuotedBrief {
            id: quoted.id.clone(),
            sender: quoted.sender.clone(),
            sender_name: quoted.sender_name.clone(),
            summary: quoted.summary.clone(),
        }),
        reactions: message
            .reactions
            .iter()
            .map(|reaction| ReactionBrief {
                emoji: reaction.emoji.clone(),
                sender: reaction.sender.clone(),
                from_me: reaction.from_me,
            })
            .collect(),
        mentions: message
            .mentions
            .iter()
            .map(|mention| mention.id.clone())
            .collect(),
    }
}

pub fn contact(contact: &Contact) -> ContactBrief {
    ContactBrief {
        id: contact.id.clone(),
        name: contact.full_name.clone().filter(|name| !name.is_empty()),
        push_name: contact.push_name.clone().filter(|name| !name.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, Reaction};

    fn text_row(text: &str) -> Message {
        Message {
            id: "3EB0".to_owned(),
            chat: "1@s.whatsapp.net".to_owned(),
            sender: "1@s.whatsapp.net".to_owned(),
            sender_name: None,
            from_me: false,
            timestamp: 1_700_000_000,
            content: Content::text(text.to_owned()),
            status: Delivery::Read,
            delivered_at: None,
            read_at: None,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    #[test]
    fn a_text_message_carries_its_body_and_a_machine_readable_time() {
        let brief = message(&text_row("see you at six"), None);
        assert_eq!(brief.kind, "text");
        assert_eq!(brief.text.as_deref(), Some("see you at six"));
        assert_eq!(brief.status, "read");
        assert_eq!(brief.at, "2023-11-14T22:13:20Z");
        assert_eq!(brief.timestamp, 1_700_000_000);
    }

    /// The archive row holds a JPEG preview. Shipping it would bloat every page
    /// of a reply and no agent asked for it.
    #[test]
    fn a_thumbnail_never_reaches_the_wire() {
        let mut row = text_row("look");
        row.thumbnail = Some(vec![0xFF, 0xD8, 0xFF, 0xE0]);
        row.content = Content::Image {
            caption: Some("look".to_owned()),
            media: Media {
                mime: "image/jpeg".to_owned(),
                size: 2048,
                width: Some(800),
                height: Some(600),
                path: None,
                state: Default::default(),
            },
        };
        let json = serde_json::to_string(&message(&row, None)).expect("json");
        assert!(!json.contains("thumbnail"), "{json}");
        assert!(json.contains("\"kind\":\"image\""), "{json}");
        assert!(
            json.contains("\"caption\"") || json.contains("\"text\":\"look\""),
            "{json}"
        );
    }

    #[test]
    fn an_undownloaded_attachment_reports_no_local_file() {
        let mut row = text_row("");
        row.content = Content::Audio {
            media: Media {
                mime: "audio/ogg".to_owned(),
                size: 9001,
                width: None,
                height: None,
                path: None,
                state: Default::default(),
            },
            seconds: Some(7),
            voice_note: true,
            waveform: vec![1, 2, 3],
        };
        let brief = message(&row, None);
        let attachment = brief.attachment.as_ref().expect("an attachment");
        assert!(attachment.voice_note);
        assert_eq!(attachment.seconds, Some(7));
        assert!(attachment.path.is_none());
        assert!(brief.text.is_none());
        // The waveform is for drawing bars, not for an agent.
        let json = serde_json::to_string(&brief).expect("json");
        assert!(!json.contains("waveform"), "{json}");
    }

    #[test]
    fn a_reaction_says_who_left_it() {
        let mut row = text_row("thanks");
        row.reactions = vec![Reaction {
            sender: "2@s.whatsapp.net".to_owned(),
            from_me: true,
            emoji: "❤️".to_owned(),
        }];
        let brief = message(&row, None);
        assert_eq!(brief.reactions.len(), 1);
        assert_eq!(brief.reactions[0].emoji, "❤️");
        assert!(brief.reactions[0].from_me);
    }

    /// A group row knows its sender's push name; a direct row does not, and the
    /// worker supplies the contact name instead.
    #[test]
    fn the_workers_name_wins_over_the_rows_own() {
        let mut row = text_row("hi");
        row.sender_name = Some("~ Push Name".to_owned());
        assert_eq!(
            message(&row, Some("Address Book".to_owned())).sender_name,
            Some("Address Book".to_owned())
        );
        assert_eq!(
            message(&row, None).sender_name,
            Some("~ Push Name".to_owned())
        );
    }

    #[test]
    fn a_gif_is_told_apart_from_a_video() {
        let media = Media {
            mime: "video/mp4".to_owned(),
            size: 1,
            width: None,
            height: None,
            path: None,
            state: Default::default(),
        };
        let gif = Content::Video {
            caption: None,
            media: media.clone(),
            seconds: Some(3),
            gif: true,
        };
        let video = Content::Video {
            caption: None,
            media,
            seconds: Some(3),
            gif: false,
        };
        assert_eq!(content_kind(&gif), "gif");
        assert_eq!(content_kind(&video), "video");
    }

    #[test]
    fn a_chat_reports_its_flags_and_its_latest_line() {
        let mut row = Chat::new("9@g.us".to_owned(), "Builders".to_owned());
        row.unread = 4;
        row.pinned = true;
        row.muted_until = Some(0);
        row.last_activity = 1_700_000_000;
        row.last = Some(crate::model::LastMessage {
            from_me: false,
            sender: "1@s.whatsapp.net".to_owned(),
            sender_name: Some("Ana".to_owned()),
            summary: "on my way".to_owned(),
            status: Delivery::None,
        });
        let brief = chat(&row);
        assert_eq!(brief.kind, "group");
        assert_eq!(brief.unread, 4);
        assert!(brief.pinned);
        assert!(brief.muted);
        assert_eq!(
            brief.last_activity_at.as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        let last = brief.last.expect("a latest line");
        assert_eq!(last.sender_name.as_deref(), Some("Ana"));
        assert_eq!(last.summary, "on my way");
    }

    #[test]
    fn an_empty_contact_name_is_left_out_rather_than_sent_blank() {
        let brief = contact(&Contact {
            id: "1@s.whatsapp.net".to_owned(),
            full_name: Some(String::new()),
            push_name: Some("Ana".to_owned()),
        });
        assert!(brief.name.is_none());
        assert_eq!(brief.push_name.as_deref(), Some("Ana"));
    }
}
