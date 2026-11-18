//! Business messages: interactive, template, buttons and list messages, and
//! the replies a user taps out of them.
//!
//! WhatsApp Business sends these for order updates, bookings and one-time
//! codes. Their text lives in a header, a body, a footer and a set of buttons
//! rather than in the plain text field, so a client that reads only that field
//! shows nothing. This flattens them into WhatsApp's own markup, which the
//! window already draws, links and all. The buttons are listed, not offered:
//! tapping one answers the business, and that is not built here.

use whatsapp_rust::prelude::{MessageField, wa};

use wa::__buffa::oneof::hydrated_template_button::HydratedButton;
use wa::__buffa::oneof::message::buttons_message::Header as ButtonsHeader;
use wa::__buffa::oneof::message::buttons_response_message::Response;
use wa::__buffa::oneof::message::interactive_message::InteractiveMessage as Kind;
use wa::__buffa::oneof::message::interactive_message::header::Media;
use wa::__buffa::oneof::message::template_message::Format;
use wa::__buffa::oneof::message::template_message::hydrated_four_row_template::Title;

/// Whether the message is one of the business kinds this module reads.
pub(super) fn is_business(base: &wa::Message) -> bool {
    base.interactive_message.is_set()
        || base.buttons_message.is_set()
        || base.list_message.is_set()
        || base.template_message.is_set()
        || base.buttons_response_message.is_set()
        || base.list_response_message.is_set()
        || base.interactive_response_message.is_set()
        || base.template_button_reply_message.is_set()
}

/// Whether a photo, video or document in the header is noted in the text or
/// shown as the message itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Header {
    Noted,
    Shown,
}

/// Readable text for a business message. `None` when it is not one, or when
/// it carries nothing a person could read, such as a bare storefront link.
pub(super) fn text(base: &wa::Message) -> Option<String> {
    compose(base, Header::Noted)
}

/// The text for a business message whose header media is shown as the
/// message itself, so it does not also name that media.
pub(super) fn caption(base: &wa::Message) -> Option<String> {
    compose(base, Header::Shown)
}

/// A plain message holding the photo, video or document a business message
/// carries in its header, so it is shown, previewed and downloaded the way
/// any other is. The header carries the same download keys as an ordinary
/// attachment. `None` when there is none, and for a carousel, whose cards
/// each carry their own.
pub(super) fn media_carrier(base: &wa::Message) -> Option<wa::Message> {
    enum Found<'a> {
        Image(&'a wa::message::ImageMessage),
        Video(&'a wa::message::VideoMessage),
        Document(&'a wa::message::DocumentMessage),
    }
    fn from_card(card: &wa::message::InteractiveMessage) -> Option<Found<'_>> {
        match card.header.as_option()?.media.as_ref()? {
            Media::ImageMessage(image) => Some(Found::Image(image)),
            Media::VideoMessage(video) => Some(Found::Video(video)),
            Media::DocumentMessage(document) => Some(Found::Document(document)),
            _ => None,
        }
    }
    fn from_title(title: &Option<Title>) -> Option<Found<'_>> {
        match title.as_ref()? {
            Title::ImageMessage(image) => Some(Found::Image(image)),
            Title::VideoMessage(video) => Some(Found::Video(video)),
            Title::DocumentMessage(document) => Some(Found::Document(document)),
            _ => None,
        }
    }
    let found = if let Some(card) = base.interactive_message.as_option() {
        match card.interactive_message {
            Some(Kind::CarouselMessage(_)) => None,
            _ => from_card(card),
        }
    } else if let Some(template) = base.template_message.as_option() {
        match &template.format {
            Some(Format::InteractiveMessageTemplate(card)) => from_card(card),
            Some(Format::HydratedFourRowTemplate(hydrated)) => from_title(&hydrated.title),
            _ => template
                .hydrated_template
                .as_option()
                .and_then(|hydrated| from_title(&hydrated.title)),
        }
    } else if let Some(buttons) = base.buttons_message.as_option() {
        match buttons.header.as_ref() {
            Some(ButtonsHeader::ImageMessage(image)) => Some(Found::Image(image)),
            Some(ButtonsHeader::VideoMessage(video)) => Some(Found::Video(video)),
            Some(ButtonsHeader::DocumentMessage(document)) => Some(Found::Document(document)),
            _ => None,
        }
    } else {
        None
    }?;
    let mut carrier = wa::Message::default();
    match found {
        Found::Image(image) => carrier.image_message = MessageField::some(image.clone()),
        Found::Video(video) => carrier.video_message = MessageField::some(video.clone()),
        Found::Document(document) => {
            carrier.document_message = MessageField::some(document.clone());
        }
    }
    Some(carrier)
}

fn compose(base: &wa::Message, header: Header) -> Option<String> {
    let rendered = if let Some(message) = base.interactive_message.as_option() {
        interactive(message, header)
    } else if let Some(message) = base.template_message.as_option() {
        template(message, header)
    } else if let Some(message) = base.buttons_message.as_option() {
        buttons(message, header)
    } else if let Some(message) = base.list_message.as_option() {
        list(message)
    } else if let Some(reply) = base.buttons_response_message.as_option() {
        match &reply.response {
            Some(Response::SelectedDisplayText(text)) => clean(text),
            _ => None,
        }
    } else if let Some(reply) = base.list_response_message.as_option() {
        join_lines([clean_opt(&reply.title), clean_opt(&reply.description)])
    } else if let Some(reply) = base.template_button_reply_message.as_option() {
        clean_opt(&reply.selected_display_text)
    } else if let Some(reply) = base.interactive_response_message.as_option() {
        reply
            .body
            .as_option()
            .and_then(|body| clean_opt(&body.text))
    } else {
        None
    };
    rendered.filter(|text| !text.trim().is_empty())
}

/// One line under a business message.
enum Line {
    /// Something to tap: a reply, a link, a code to copy.
    Button(String),
    /// A section heading in a list.
    Heading(String),
    /// A row inside a menu that a button opens.
    Row(String),
}

/// One business message laid out as a header, body, footer and buttons.
#[derive(Default)]
struct Card {
    /// What the header carried that is not drawn here, such as a photo.
    media: Option<&'static str>,
    title: Option<String>,
    subtitle: Option<String>,
    body: Option<String>,
    footer: Option<String>,
    buttons: Vec<Line>,
}

impl Card {
    /// WhatsApp markup: a bold title, the body as sent, an italic footer, one
    /// line per button, and a note of any header media last unless it is shown.
    /// The chat list previews the first line, which should be the title or the
    /// body.
    fn render(&self, header: Header) -> Option<String> {
        let head = join_lines([
            self.title.as_ref().map(|title| format!("*{title}*")),
            self.subtitle.clone(),
        ]);
        let buttons = (!self.buttons.is_empty()).then(|| {
            self.buttons
                .iter()
                .map(|line| match line {
                    Line::Button(text) => format!("• {text}"),
                    Line::Heading(text) => format!("*{text}*"),
                    Line::Row(text) => format!("    – {text}"),
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
        let blocks: Vec<String> = [
            head,
            self.body.clone(),
            self.footer.as_ref().map(|footer| format!("_{footer}_")),
            buttons,
            self.media
                .filter(|_| header == Header::Noted)
                .map(|media| format!("[{media}]")),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!blocks.is_empty()).then(|| blocks.join("\n\n"))
    }
}

fn interactive(message: &wa::message::InteractiveMessage, header: Header) -> Option<String> {
    let mut card = Card::default();
    if let Some(header) = message.header.as_option() {
        card.title = clean_opt(&header.title);
        card.subtitle = clean_opt(&header.subtitle);
        card.media = header.media.as_ref().and_then(media_label);
    }
    card.body = message
        .body
        .as_option()
        .and_then(|body| clean_opt(&body.text));
    card.footer = message
        .footer
        .as_option()
        .and_then(|footer| clean_opt(&footer.text));
    let mut cards = Vec::new();
    match &message.interactive_message {
        Some(Kind::NativeFlowMessage(flow)) => {
            card.buttons = flow
                .buttons
                .iter()
                .flat_map(|button| {
                    native_flow(button.name.as_deref(), button.button_params_json.as_deref())
                })
                .collect();
        }
        // A carousel is a row of cards that each read like a message. Their
        // pictures are not shown, so each card notes its own.
        Some(Kind::CarouselMessage(carousel)) => {
            cards = carousel
                .cards
                .iter()
                .filter_map(|card| interactive(card, Header::Noted))
                .collect();
        }
        _ => {}
    }
    let blocks: Vec<String> = card.render(header).into_iter().chain(cards).collect();
    (!blocks.is_empty()).then(|| blocks.join("\n\n———\n\n"))
}

fn template(message: &wa::message::TemplateMessage, header: Header) -> Option<String> {
    match &message.format {
        Some(Format::InteractiveMessageTemplate(inner)) => return interactive(inner, header),
        Some(Format::HydratedFourRowTemplate(hydrated)) => {
            return hydrated_template(hydrated, header);
        }
        _ => {}
    }
    // The unhydrated form needs the business's localized strings, which do
    // not travel with the message. The hydrated copy beside it carries text.
    message
        .hydrated_template
        .as_option()
        .and_then(|hydrated| hydrated_template(hydrated, header))
}

fn hydrated_template(
    template: &wa::message::template_message::HydratedFourRowTemplate,
    header: Header,
) -> Option<String> {
    let mut card = Card {
        body: clean_opt(&template.hydrated_content_text),
        footer: clean_opt(&template.hydrated_footer_text),
        ..Card::default()
    };
    match &template.title {
        Some(Title::HydratedTitleText(title)) => card.title = clean(title),
        Some(Title::ImageMessage(_)) => card.media = Some("Photo"),
        Some(Title::VideoMessage(_)) => card.media = Some("Video"),
        Some(Title::DocumentMessage(_)) => card.media = Some("Document"),
        Some(Title::LocationMessage(_)) => card.media = Some("Location"),
        _ => {}
    }
    card.buttons = template
        .hydrated_buttons
        .iter()
        .filter_map(|button| match &button.hydrated_button {
            Some(HydratedButton::QuickReplyButton(reply)) => clean_opt(&reply.display_text),
            Some(HydratedButton::UrlButton(link)) => {
                labelled(clean_opt(&link.display_text), clean_opt(&link.url))
            }
            Some(HydratedButton::CallButton(call)) => {
                labelled(clean_opt(&call.display_text), clean_opt(&call.phone_number))
            }
            _ => None,
        })
        .map(Line::Button)
        .collect();
    card.render(header)
}

fn buttons(message: &wa::message::ButtonsMessage, header: Header) -> Option<String> {
    let mut card = Card {
        body: clean_opt(&message.content_text),
        footer: clean_opt(&message.footer_text),
        ..Card::default()
    };
    match &message.header {
        Some(ButtonsHeader::Text(title)) => card.title = clean(title),
        Some(ButtonsHeader::ImageMessage(_)) => card.media = Some("Photo"),
        Some(ButtonsHeader::VideoMessage(_)) => card.media = Some("Video"),
        Some(ButtonsHeader::DocumentMessage(_)) => card.media = Some("Document"),
        Some(ButtonsHeader::LocationMessage(_)) => card.media = Some("Location"),
        _ => {}
    }
    card.buttons = message
        .buttons
        .iter()
        .flat_map(|button| {
            let shown = button
                .button_text
                .as_option()
                .and_then(|text| clean_opt(&text.display_text));
            match (shown, button.native_flow_info.as_option()) {
                (Some(shown), _) => vec![Line::Button(shown)],
                (None, Some(flow)) => {
                    native_flow(flow.name.as_deref(), flow.params_json.as_deref())
                }
                (None, None) => Vec::new(),
            }
        })
        .collect();
    card.render(header)
}

fn list(message: &wa::message::ListMessage) -> Option<String> {
    let mut card = Card {
        title: clean_opt(&message.title),
        body: clean_opt(&message.description),
        footer: clean_opt(&message.footer_text),
        ..Card::default()
    };
    for section in &message.sections {
        if let Some(title) = clean_opt(&section.title) {
            card.buttons.push(Line::Heading(title));
        }
        card.buttons.extend(
            section
                .rows
                .iter()
                .filter_map(|row| labelled(clean_opt(&row.title), clean_opt(&row.description)))
                .map(Line::Button),
        );
    }
    // A list with no rows still names the menu it would open.
    if card.buttons.is_empty() {
        card.buttons
            .extend(clean_opt(&message.button_text).map(Line::Button));
    }
    card.render(Header::Noted)
}

/// Lines for one native-flow button. Its label and what it points at live in a
/// JSON string; the keys below are the ones businesses send.
fn native_flow(name: Option<&str>, params: Option<&str>) -> Vec<Line> {
    let params: serde_json::Value = params
        .and_then(|params| serde_json::from_str(params).ok())
        .unwrap_or_default();
    let field = |key: &str| {
        params
            .get(key)
            .and_then(|value| value.as_str())
            .and_then(clean)
    };
    let label = field("display_text").or_else(|| field("title"));
    // A one-time code, a link or a phone number is the part a person needs,
    // so it is shown beside the label rather than hidden behind it.
    let detail = field("copy_code")
        .or_else(|| field("url"))
        .or_else(|| field("phone_number"));
    let mut lines: Vec<Line> = labelled(label, detail)
        .map(Line::Button)
        .into_iter()
        .collect();
    // A single-select button opens a menu of rows, which read like a list.
    if name == Some("single_select")
        && let Some(sections) = params.get("sections").and_then(|value| value.as_array())
    {
        for row in sections
            .iter()
            .filter_map(|section| section.get("rows")?.as_array())
            .flatten()
        {
            let title = row
                .get("title")
                .and_then(|value| value.as_str())
                .and_then(clean);
            let description = row
                .get("description")
                .and_then(|value| value.as_str())
                .and_then(clean);
            lines.extend(labelled(title, description).map(Line::Row));
        }
    }
    lines
}

fn media_label(media: &Media) -> Option<&'static str> {
    match media {
        Media::ImageMessage(_) | Media::JpegThumbnail(_) => Some("Photo"),
        Media::VideoMessage(_) => Some("Video"),
        Media::DocumentMessage(_) => Some("Document"),
        Media::LocationMessage(_) => Some("Location"),
        Media::ProductMessage(_) => Some("Product"),
        _ => None,
    }
}

/// `label: detail`, either half alone, or nothing.
fn labelled(label: Option<String>, detail: Option<String>) -> Option<String> {
    match (label, detail) {
        (Some(label), Some(detail)) if label != detail => Some(format!("{label}: {detail}")),
        (Some(label), _) => Some(label),
        (None, detail) => detail,
    }
}

fn join_lines<const N: usize>(lines: [Option<String>; N]) -> Option<String> {
    let lines: Vec<String> = lines.into_iter().flatten().collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn clean(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn clean_opt(text: &Option<String>) -> Option<String> {
    text.as_deref().and_then(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::proto;
    use whatsapp_rust::prelude::{MessageExt, MessageField};

    use wa::message::interactive_message as im;

    type Button = im::native_flow_message::NativeFlowButton;

    fn native(name: &str, params: serde_json::Value) -> Button {
        proto(|button: &mut Button| {
            button.name = Some(name.to_owned());
            button.button_params_json = Some(params.to_string());
        })
    }

    fn interactive_card(
        title: Option<&str>,
        body: &str,
        footer: Option<&str>,
        buttons: Vec<Button>,
    ) -> wa::message::InteractiveMessage {
        proto(|message: &mut wa::message::InteractiveMessage| {
            if let Some(title) = title {
                message.header = MessageField::some(proto(|header: &mut im::Header| {
                    header.title = Some(title.to_owned());
                }));
            }
            message.body = MessageField::some(proto(|text: &mut im::Body| {
                text.text = Some(body.to_owned());
            }));
            if let Some(footer) = footer {
                message.footer = MessageField::some(proto(|text: &mut im::Footer| {
                    text.text = Some(footer.to_owned());
                }));
            }
            if !buttons.is_empty() {
                let flow = proto(|flow: &mut im::NativeFlowMessage| flow.buttons = buttons);
                message.interactive_message = Some(Kind::NativeFlowMessage(Box::new(flow)));
            }
        })
    }

    fn holding(interactive: wa::message::InteractiveMessage) -> wa::Message {
        proto(|message: &mut wa::Message| {
            message.interactive_message = MessageField::some(interactive);
        })
    }

    /// The common shape: a bank or a courier with a title, a body, a footer,
    /// a link and a one-time code.
    #[test]
    fn a_business_message_reads_as_title_body_footer_and_buttons() {
        let message = holding(interactive_card(
            Some("Your order has shipped"),
            "Arriving Thursday between 9 and 12.",
            Some("Reply STOP to opt out"),
            vec![
                native(
                    "cta_url",
                    serde_json::json!({ "display_text": "Track", "url": "https://example.test/t/1" }),
                ),
                native(
                    "cta_copy",
                    serde_json::json!({ "display_text": "Copy code", "copy_code": "482913" }),
                ),
            ],
        ));
        assert!(is_business(&message));
        assert_eq!(
            text(&message).expect("readable"),
            "*Your order has shipped*\n\n\
             Arriving Thursday between 9 and 12.\n\n\
             _Reply STOP to opt out_\n\n\
             • Track: https://example.test/t/1\n\
             • Copy code: 482913"
        );
    }

    /// The chat list previews the first line, so a photo header must not push
    /// the body down behind a placeholder.
    #[test]
    fn a_photo_header_is_noted_last_so_the_preview_shows_the_words() {
        let mut card = interactive_card(None, "Your table is booked for 8pm.", None, Vec::new());
        card.header = MessageField::some(proto(|header: &mut im::Header| {
            header.media = Some(Media::JpegThumbnail(vec![0xFF, 0xD8]));
        }));
        let rendered = text(&holding(card)).expect("readable");
        assert_eq!(
            rendered.lines().next(),
            Some("Your table is booked for 8pm.")
        );
        assert!(rendered.ends_with("[Photo]"), "{rendered}");
    }

    fn photo() -> wa::message::ImageMessage {
        proto(|image: &mut wa::message::ImageMessage| {
            image.mimetype = Some("image/jpeg".to_owned());
            image.file_length = Some(48_000);
            image.width = Some(800);
            image.height = Some(600);
            image.jpeg_thumbnail = Some(vec![0xFF, 0xD8, 0x01]);
            image.direct_path = Some("/v/t62/abc".to_owned());
            image.media_key = Some(vec![7; 32]);
        })
    }

    fn with_photo_header(card: &mut wa::message::InteractiveMessage) {
        let header = card.header.get_or_insert_default();
        header.media = Some(Media::ImageMessage(Box::new(photo())));
    }

    /// The header holds a whole photo with its download keys, so it becomes
    /// the message, captioned with the business's words, and is downloaded
    /// like any other photo.
    #[test]
    fn a_header_photo_becomes_the_message_with_the_words_as_its_caption() {
        use crate::model::Content;
        let mut card = interactive_card(Some("Sale"), "Half price today", None, Vec::new());
        with_photo_header(&mut card);
        let message = holding(card);

        let carrier = media_carrier(&message).expect("a photo to carry");
        assert_eq!(carrier.image_message.as_option(), Some(&photo()));
        assert!(!carrier.interactive_message.is_set());

        match super::super::classify(&message) {
            Some(Content::Image { caption, media }) => {
                assert_eq!(caption.as_deref(), Some("*Sale*\n\nHalf price today"));
                assert_eq!(media.mime, "image/jpeg");
            }
            other => panic!("expected a photo, got {other:?}"),
        }
        // The note is for when the photo is not shown; here it is.
        assert!(text(&message).expect("text").ends_with("[Photo]"));
        assert!(!caption(&message).expect("caption").contains("[Photo]"));
    }

    #[test]
    fn a_header_photo_brings_its_preview_before_it_downloads() {
        let mut card = interactive_card(None, "Hi", None, Vec::new());
        with_photo_header(&mut card);
        assert_eq!(
            super::super::thumbnail_of(&holding(card)),
            Some(vec![0xFF, 0xD8, 0x01])
        );
    }

    #[test]
    fn templates_and_buttons_carry_their_header_photos_too() {
        use wa::message::template_message::HydratedFourRowTemplate;
        let template = proto(|message: &mut wa::Message| {
            message.template_message =
                MessageField::some(proto(|template: &mut wa::message::TemplateMessage| {
                    template.hydrated_template =
                        MessageField::some(proto(|hydrated: &mut HydratedFourRowTemplate| {
                            hydrated.title = Some(Title::ImageMessage(Box::new(photo())));
                            hydrated.hydrated_content_text = Some("Ticket".to_owned());
                        }));
                }));
        });
        assert!(media_carrier(&template).is_some());

        let buttons = proto(|message: &mut wa::Message| {
            message.buttons_message =
                MessageField::some(proto(|buttons: &mut wa::message::ButtonsMessage| {
                    buttons.header = Some(ButtonsHeader::ImageMessage(Box::new(photo())));
                    buttons.content_text = Some("Pick one".to_owned());
                }));
        });
        assert!(media_carrier(&buttons).is_some());
    }

    /// Each card in a carousel has its own picture; none of them is the
    /// message.
    #[test]
    fn a_carousel_is_not_turned_into_one_of_its_pictures() {
        let mut first = interactive_card(Some("A"), "a", None, Vec::new());
        with_photo_header(&mut first);
        let carousel = proto(|carousel: &mut im::CarouselMessage| carousel.cards = vec![first]);
        let mut outer = interactive_card(None, "Pick", None, Vec::new());
        outer.interactive_message = Some(Kind::CarouselMessage(Box::new(carousel)));
        assert!(media_carrier(&holding(outer)).is_none());
    }

    /// A bare preview in the header has no download keys, so there is nothing
    /// to show beyond the note.
    #[test]
    fn a_header_with_only_a_preview_stays_text_with_a_note() {
        let mut card = interactive_card(None, "Hi", None, Vec::new());
        card.header.get_or_insert_default().media = Some(Media::JpegThumbnail(vec![0xFF]));
        let message = holding(card);
        assert!(media_carrier(&message).is_none());
        assert!(text(&message).expect("text").ends_with("[Photo]"));
    }

    #[test]
    fn a_forwarded_business_photo_goes_out_as_the_photo_with_its_caption() {
        use crate::model::Content;
        let mut card = interactive_card(Some("Sale"), "Half price", None, Vec::new());
        with_photo_header(&mut card);
        let shown = Content::Image {
            caption: Some("*Sale*\n\nHalf price".to_owned()),
            media: crate::model::Media {
                mime: "image/jpeg".to_owned(),
                size: 48_000,
                width: Some(800),
                height: Some(600),
                path: None,
                state: Default::default(),
            },
        };
        let out = super::super::forwardable(holding(card), &shown);
        assert!(!out.interactive_message.is_set());
        let image = out.image_message.as_option().expect("the photo");
        assert_eq!(image.caption.as_deref(), Some("*Sale*\n\nHalf price"));
        assert_eq!(image.media_key, photo().media_key);
    }

    #[test]
    fn a_carousel_reads_as_its_cards_in_order() {
        let carousel = proto(|carousel: &mut im::CarouselMessage| {
            carousel.cards = vec![
                interactive_card(Some("Room A"), "Sea view", None, Vec::new()),
                interactive_card(Some("Room B"), "Garden view", None, Vec::new()),
            ];
        });
        let mut outer = interactive_card(None, "Pick a room", None, Vec::new());
        outer.interactive_message = Some(Kind::CarouselMessage(Box::new(carousel)));
        let rendered = text(&holding(outer)).expect("readable");
        let a = rendered.find("Room A").expect("first card");
        let b = rendered.find("Room B").expect("second card");
        assert!(rendered.starts_with("Pick a room"), "{rendered}");
        assert!(a < b, "{rendered}");
    }

    #[test]
    fn a_hydrated_template_lists_its_link_call_and_reply_buttons() {
        use wa::message::template_message::HydratedFourRowTemplate;
        let button = |fill: HydratedButton| {
            proto(|button: &mut wa::HydratedTemplateButton| button.hydrated_button = Some(fill))
        };
        let hydrated = proto(|template: &mut HydratedFourRowTemplate| {
            template.title = Some(Title::HydratedTitleText("Appointment".to_owned()));
            template.hydrated_content_text = Some("Tomorrow at 10:30".to_owned());
            template.hydrated_buttons = vec![
                button(HydratedButton::UrlButton(Box::new(proto(
                    |link: &mut wa::hydrated_template_button::HydratedURLButton| {
                        link.display_text = Some("Directions".to_owned());
                        link.url = Some("https://example.test/map".to_owned());
                    },
                )))),
                button(HydratedButton::CallButton(Box::new(proto(
                    |call: &mut wa::hydrated_template_button::HydratedCallButton| {
                        call.display_text = Some("Call clinic".to_owned());
                        call.phone_number = Some("+15550000000".to_owned());
                    },
                )))),
                button(HydratedButton::QuickReplyButton(Box::new(proto(
                    |reply: &mut wa::hydrated_template_button::HydratedQuickReplyButton| {
                        reply.display_text = Some("Confirm".to_owned());
                    },
                )))),
            ];
        });
        let message = proto(|message: &mut wa::Message| {
            message.template_message =
                MessageField::some(proto(|template: &mut wa::message::TemplateMessage| {
                    template.hydrated_template = MessageField::some(hydrated);
                }));
        });
        assert_eq!(
            text(&message).expect("readable"),
            "*Appointment*\n\n\
             Tomorrow at 10:30\n\n\
             • Directions: https://example.test/map\n\
             • Call clinic: +15550000000\n\
             • Confirm"
        );
    }

    #[test]
    fn a_list_shows_its_sections_and_rows() {
        use wa::message::list_message::{Row, Section};
        let message = proto(|message: &mut wa::Message| {
            message.list_message =
                MessageField::some(proto(|list: &mut wa::message::ListMessage| {
                    list.title = Some("Menu".to_owned());
                    list.description = Some("What would you like?".to_owned());
                    list.button_text = Some("Open menu".to_owned());
                    list.sections = vec![proto(|section: &mut Section| {
                        section.title = Some("Drinks".to_owned());
                        section.rows = vec![
                            proto(|row: &mut Row| {
                                row.title = Some("Tea".to_owned());
                                row.description = Some("Hot".to_owned());
                            }),
                            proto(|row: &mut Row| row.title = Some("Juice".to_owned())),
                        ];
                    })];
                }));
        });
        assert_eq!(
            text(&message).expect("readable"),
            "*Menu*\n\nWhat would you like?\n\n*Drinks*\n• Tea: Hot\n• Juice"
        );
    }

    #[test]
    fn a_buttons_message_takes_its_title_from_a_text_header() {
        use wa::message::buttons_message::{Button as Choice, button::ButtonText};
        let choice = |label: &str| {
            proto(|button: &mut Choice| {
                button.button_text = MessageField::some(proto(|text: &mut ButtonText| {
                    text.display_text = Some(label.to_owned());
                }));
            })
        };
        let message = proto(|message: &mut wa::Message| {
            message.buttons_message =
                MessageField::some(proto(|buttons: &mut wa::message::ButtonsMessage| {
                    buttons.header = Some(ButtonsHeader::Text("Survey".to_owned()));
                    buttons.content_text = Some("Was delivery on time?".to_owned());
                    buttons.buttons = vec![choice("Yes"), choice("No")];
                }));
        });
        assert_eq!(
            text(&message).expect("readable"),
            "*Survey*\n\nWas delivery on time?\n\n• Yes\n• No"
        );
    }

    /// What the user tapped reads as the words on the button.
    #[test]
    fn a_reply_to_a_business_message_reads_as_the_choice() {
        let reply = proto(|message: &mut wa::Message| {
            message.buttons_response_message =
                MessageField::some(proto(|reply: &mut wa::message::ButtonsResponseMessage| {
                    reply.response = Some(Response::SelectedDisplayText("Yes".to_owned()));
                }));
        });
        assert_eq!(text(&reply).as_deref(), Some("Yes"));

        let picked = proto(|message: &mut wa::Message| {
            message.template_button_reply_message = MessageField::some(proto(
                |reply: &mut wa::message::TemplateButtonReplyMessage| {
                    reply.selected_display_text = Some("Confirm".to_owned());
                },
            ));
        });
        assert_eq!(text(&picked).as_deref(), Some("Confirm"));
    }

    /// A message with nothing to read stays "unsupported" rather than becoming
    /// an empty bubble.
    #[test]
    fn a_business_message_with_no_words_is_left_for_the_fallback() {
        let empty = holding(proto(|message: &mut wa::message::InteractiveMessage| {
            message.interactive_message = Some(Kind::ShopStorefrontMessage(Box::default()));
        }));
        assert!(is_business(&empty));
        assert!(text(&empty).is_none());
        assert!(!is_business(&wa::Message::default()));
    }

    /// The archive files what `classify` returns, so this is the path a
    /// message actually takes into the window.
    #[test]
    fn classify_files_a_business_message_as_text_and_an_empty_one_as_unsupported() {
        use crate::model::Content;
        let readable = holding(interactive_card(Some("Hi"), "Body", None, Vec::new()));
        match super::super::classify(&readable) {
            Some(Content::Text { text, .. }) => assert_eq!(text, "*Hi*\n\nBody"),
            other => panic!("expected text, got {other:?}"),
        }
        let empty = holding(proto(|message: &mut wa::message::InteractiveMessage| {
            message.interactive_message = Some(Kind::ShopStorefrontMessage(Box::default()));
        }));
        assert!(matches!(
            super::super::classify(&empty),
            Some(Content::Unsupported { .. })
        ));
    }

    /// Forwarding re-sends the original protobuf, which for a business
    /// message would carry buttons a personal account cannot send.
    #[test]
    fn a_forwarded_business_message_goes_out_as_the_text_shown() {
        use crate::model::Content;
        let original = holding(interactive_card(Some("Hi"), "Body", None, Vec::new()));
        let shown = Content::text("*Hi*\n\nBody".to_owned());
        let out = super::super::forwardable(original, &shown);
        assert!(!out.interactive_message.is_set());
        assert_eq!(out.get_base_message().text_content(), Some("*Hi*\n\nBody"));

        // Anything else is forwarded as it came.
        let plain = proto(|message: &mut wa::Message| {
            message.conversation = Some("hello".to_owned());
        });
        let out = super::super::forwardable(plain.clone(), &Content::text("hello".to_owned()));
        assert_eq!(out, plain);
    }

    /// The parameters are a JSON string the business writes. A broken one
    /// loses its button, not the message.
    #[test]
    fn a_button_with_unreadable_parameters_is_dropped_quietly() {
        let mut broken = native("cta_url", serde_json::json!({}));
        broken.button_params_json = Some("{not json".to_owned());
        let message = holding(interactive_card(None, "Hello", None, vec![broken]));
        assert_eq!(text(&message).as_deref(), Some("Hello"));
    }

    #[test]
    fn a_single_select_button_lists_the_rows_it_would_open() {
        let menu = native(
            "single_select",
            serde_json::json!({
                "title": "Choose a slot",
                "sections": [{ "rows": [
                    { "title": "Morning", "description": "9 to 12" },
                    { "title": "Evening" }
                ]}]
            }),
        );
        let message = holding(interactive_card(None, "Delivery slot", None, vec![menu]));
        assert_eq!(
            text(&message).expect("readable"),
            "Delivery slot\n\n• Choose a slot\n    – Morning: 9 to 12\n    – Evening"
        );
    }
}
