//! Answers for the local agent API.
//!
//! The worker holds the only connection to the encrypted archive, so these run
//! on its thread between commands. Every answer is a [`wire`] shape built here;
//! nothing in this file touches the interface or the settings file.

use crate::agent::{Call, Failure, Reply, Request, wire};
use crate::model::ChatId;

use super::Worker;

impl Worker {
    /// Gates the request, answers it, and sends the result back.
    pub(super) fn handle_agent(&mut self, call: Call) {
        let method = call.request.method();
        let reply = self
            .agent_policy
            .gate(&call.request)
            .and_then(|()| self.answer_agent(call.request));
        // Method and outcome only. Chat ids and message text are personal data.
        match &reply {
            Ok(_) => log::info!("agent: {method} ok"),
            Err(failure) => log::info!("agent: {method} {}", failure.code()),
        }
        let _ = call.reply.send(reply);
    }

    fn answer_agent(&mut self, request: Request) -> Reply {
        match request {
            Request::Status => self.agent_status(),
            Request::ListChats {
                limit,
                query,
                unread_only,
                include_archived,
            } => self.agent_list_chats(limit, query, unread_only, include_archived),
            Request::GetChat { chat } => self.agent_get_chat(&chat),
            Request::GetMessages {
                chat,
                limit,
                before,
            } => self.agent_get_messages(&chat, limit, before),
            Request::SearchMessages { query, limit } => self.agent_search(&query, limit),
            Request::ListContacts { query, limit } => self.agent_list_contacts(query, limit),
            Request::SendText {
                chat,
                text,
                quoting,
                mentions,
            } => self.agent_send_text(chat, text, quoting, mentions),
            Request::React {
                chat,
                message,
                emoji,
            } => self.agent_react(chat, message, emoji),
            Request::MarkRead { chat } => self.agent_mark_read(chat),
        }
    }

    // --- reads -----------------------------------------------------------

    fn agent_status(&self) -> Reply {
        let status = wire::Status {
            app: crate::profile::Profile::current().display_name(),
            version: env!("CARGO_PKG_VERSION"),
            link: self.status.log_label(),
            connected: self.status.is_connected(),
            syncing: self.syncing,
            me: self.me_pn.clone().or_else(|| self.me_lid.clone()),
            me_name: self.me_name.clone(),
            chats: self.archive.chat_count().map_err(internal)?,
            can_send: self.agent_policy.may_send(),
            sends_left: self.agent_budget.left(),
        };
        json(&status)
    }

    fn agent_list_chats(
        &self,
        limit: usize,
        query: Option<String>,
        unread_only: bool,
        include_archived: bool,
    ) -> Reply {
        let needle = query.as_deref().map(crate::util::search_key);
        let mut chats: Vec<_> = self
            .archive
            .chats()
            .map_err(internal)?
            .into_iter()
            .filter(|chat| include_archived || !chat.archived)
            .filter(|chat| !unread_only || chat.unread > 0)
            .filter(|chat| match &needle {
                None => true,
                Some(needle) => {
                    crate::util::search_key(&chat.name).contains(needle)
                        || chat.phone().is_some_and(|phone| phone.contains(needle))
                }
            })
            .collect();
        // Pinned first, newest within each group. The same order the window shows.
        chats.sort_by(|a, b| {
            b.pinned.cmp(&a.pinned).then_with(|| {
                if a.pinned && b.pinned {
                    b.pinned_at.cmp(&a.pinned_at).then(a.id.cmp(&b.id))
                } else {
                    b.last_activity.cmp(&a.last_activity).then(a.id.cmp(&b.id))
                }
            })
        });
        let total = chats.len();
        let page: Vec<_> = chats.iter().take(limit).map(wire::chat).collect();
        json(&serde_json::json!({ "chats": page, "total": total }))
    }

    fn agent_get_chat(&self, chat: &str) -> Reply {
        let row = self.chat_or_missing(chat)?;
        let mut brief = serde_json::to_value(wire::chat(&row)).map_err(internal)?;
        // Group membership is not in the chat list, and an agent writing to a
        // group wants to know who is in it.
        if let Some(object) = brief.as_object_mut() {
            let members: Vec<_> = row
                .participants
                .iter()
                .map(|id| serde_json::json!({ "id": id, "name": self.name_for(id) }))
                .collect();
            object.insert("members".to_owned(), serde_json::Value::Array(members));
        }
        Ok(brief)
    }

    fn agent_get_messages(
        &self,
        chat: &str,
        limit: usize,
        before: Option<crate::agent::Cursor>,
    ) -> Reply {
        self.chat_or_missing(chat)?;
        let boundary = before
            .as_ref()
            .map(|cursor| (cursor.timestamp, cursor.id.as_str()));
        let rows = self
            .archive
            .messages(chat, boundary, limit)
            .map_err(internal)?;
        // Rows arrive oldest first. The next page starts before the earliest.
        let next = (rows.len() == limit)
            .then(|| rows.first())
            .flatten()
            .map(|row| serde_json::json!({ "timestamp": row.timestamp, "id": row.id }));
        let messages: Vec<_> = rows
            .iter()
            .map(|row| wire::message(row, self.sender_name(row)))
            .collect();
        json(&serde_json::json!({
            "chat": chat,
            "messages": messages,
            "next_before": next,
        }))
    }

    fn agent_search(&self, query: &str, limit: usize) -> Reply {
        let rows = self
            .archive
            .search_messages(query, limit)
            .map_err(internal)?;
        let messages: Vec<_> = rows
            .iter()
            .map(|row| {
                let mut brief = wire::message(row, self.sender_name(row));
                brief.chat_name = self
                    .archive
                    .chat(&row.chat)
                    .ok()
                    .flatten()
                    .map(|chat| chat.name);
                brief
            })
            .collect();
        json(&serde_json::json!({ "query": query, "messages": messages }))
    }

    fn agent_list_contacts(&self, query: Option<String>, limit: usize) -> Reply {
        let needle = query.as_deref().map(crate::util::search_key);
        let mut contacts: Vec<_> = self
            .archive
            .contacts()
            .map_err(internal)?
            .into_iter()
            .filter(|contact| match &needle {
                None => true,
                Some(needle) => {
                    contact
                        .display_name()
                        .is_some_and(|name| crate::util::search_key(name).contains(needle))
                        || contact.id.contains(needle.as_str())
                }
            })
            .collect();
        contacts
            .sort_by_key(|contact| contact.display_name().unwrap_or(&contact.id).to_lowercase());
        let total = contacts.len();
        let page: Vec<_> = contacts.iter().take(limit).map(wire::contact).collect();
        json(&serde_json::json!({ "contacts": page, "total": total }))
    }

    // --- writes ----------------------------------------------------------

    fn agent_send_text(
        &mut self,
        chat: ChatId,
        text: String,
        quoting: Option<String>,
        mentions: Vec<String>,
    ) -> Reply {
        let row = self.chat_or_missing(&chat)?;
        if row.read_only {
            return Err(Failure::Refused(
                "this is an announcement group and nobody but its admins can post".to_owned(),
            ));
        }
        // A quote that silently vanishes is worse than a refusal. Checked
        // before the link, so a malformed call is reported as malformed rather
        // than as something to retry later.
        if let Some(id) = quoting.as_deref()
            && self.archive.message(&chat, id).map_err(internal)?.is_none()
        {
            return Err(Failure::NotFound("quoted message".to_owned()));
        }
        if self.client.is_none() {
            return Err(Failure::Offline);
        }
        self.agent_budget.take()?;
        let Some(id) = self.send_text(chat.clone(), text, quoting, mentions) else {
            return Err(Failure::Offline);
        };
        json(&wire::Accepted {
            chat,
            message: id,
            status: "pending",
        })
    }

    fn agent_react(&mut self, chat: ChatId, message: String, emoji: String) -> Reply {
        self.chat_or_missing(&chat)?;
        if self
            .archive
            .message(&chat, &message)
            .map_err(internal)?
            .is_none()
        {
            return Err(Failure::NotFound("message".to_owned()));
        }
        if self.client.is_none() {
            return Err(Failure::Offline);
        }
        self.agent_budget.take()?;
        self.react(chat.clone(), message.clone(), emoji);
        json(&wire::Accepted {
            chat,
            message,
            status: "pending",
        })
    }

    /// Clears the chat's unread count, and tells the sender when the user's own
    /// read-receipt setting permits it.
    fn agent_mark_read(&mut self, chat: ChatId) -> Reply {
        self.chat_or_missing(&chat)?;
        let receipts = self.agent_policy.read_receipts();
        self.mark_read(chat.clone(), receipts);
        json(&serde_json::json!({ "chat": chat, "receipts": receipts }))
    }

    // --- helpers ---------------------------------------------------------

    fn chat_or_missing(&self, chat: &str) -> Result<crate::model::Chat, Failure> {
        self.archive
            .chat(chat)
            .map_err(internal)?
            .ok_or_else(|| Failure::NotFound("chat".to_owned()))
    }

    /// Best name for a row's sender. The archive only stores one for groups.
    fn sender_name(&self, row: &crate::model::Message) -> Option<String> {
        self.name_for(&row.sender)
    }
}

fn internal(error: impl std::fmt::Display) -> Failure {
    Failure::Internal(error.to_string())
}

fn json<T: serde::Serialize>(value: &T) -> Reply {
    serde_json::to_value(value).map_err(internal)
}

#[cfg(test)]
mod tests {
    use super::super::receipt_tests::{ME, PEER, worker};
    use super::*;
    use crate::agent::{Call, Policy};
    use crate::model::{Content, Delivery, Message};

    fn ask(worker: &mut Worker, method: &str, arguments: serde_json::Value) -> Reply {
        let request = Request::from_call(method, arguments).expect("a request");
        let (reply, mut receiver) = tokio::sync::oneshot::channel();
        worker.handle_agent(Call { request, reply });
        receiver.try_recv().expect("an answer on the channel")
    }

    fn row(chat: &str, id: &str, timestamp: i64, text: &str) -> Message {
        Message {
            id: id.to_owned(),
            chat: chat.to_owned(),
            sender: PEER.to_owned(),
            sender_name: None,
            from_me: false,
            timestamp,
            content: Content::text(text.to_owned()),
            status: Delivery::None,
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
    fn the_chat_list_puts_pinned_first_and_newest_after() {
        let (mut worker, _events, _inbox, _wa) = worker();
        let group = "1-2@g.us";
        for (id, name) in [(PEER, "Ana"), (group, "Builders")] {
            worker.archive.ensure_chat(id, name).expect("a chat");
        }
        // The pinned chat is the older one, so only the pin can lift it.
        worker
            .archive
            .insert_message(&row(group, "A", 100, "older"), None)
            .expect("stored");
        worker
            .archive
            .insert_message(&row(PEER, "B", 900, "newer"), None)
            .expect("stored");
        worker
            .archive
            .set_pinned_at(group, true, 5)
            .expect("pinned");

        let answer = ask(&mut worker, "list_chats", serde_json::json!({})).expect("a page");
        let chats = answer["chats"].as_array().expect("an array");
        assert_eq!(answer["total"], 2);
        assert_eq!(chats[0]["id"], group);
        assert_eq!(chats[0]["pinned"], true);
        assert_eq!(chats[1]["id"], PEER);
    }

    #[test]
    fn an_unread_filter_and_a_name_search_narrow_the_same_list() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker
            .archive
            .ensure_chat(PEER, "Ana Lopez")
            .expect("a chat");
        worker
            .archive
            .ensure_chat("1-2@g.us", "Builders")
            .expect("a chat");
        worker.archive.set_unread(PEER, 3).expect("unread");

        let unread = ask(
            &mut worker,
            "list_chats",
            serde_json::json!({ "unread_only": true }),
        )
        .expect("a page");
        assert_eq!(unread["total"], 1);
        assert_eq!(unread["chats"][0]["unread"], 3);

        // Accent- and case-insensitive, the same as the window's search field.
        let found = ask(
            &mut worker,
            "list_chats",
            serde_json::json!({ "query": "ANA" }),
        )
        .expect("a page");
        assert_eq!(found["total"], 1);
        assert_eq!(found["chats"][0]["name"], "Ana Lopez");
    }

    /// An agent that mistypes a chat id must be told, not handed an empty page
    /// it would read as "this conversation is empty".
    #[test]
    fn a_chat_that_is_not_here_is_reported_missing() {
        let (mut worker, _events, _inbox, _wa) = worker();
        for method in ["get_chat", "get_messages", "mark_read"] {
            let failure = ask(
                &mut worker,
                method,
                serde_json::json!({ "chat": "99@s.whatsapp.net" }),
            )
            .expect_err("a rejection");
            assert_eq!(failure.code(), "not_found", "{method}");
            assert_eq!(failure.status(), 404, "{method}");
        }
    }

    #[test]
    fn messages_come_back_oldest_first_with_a_cursor_for_the_page_before() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "Ana").expect("a chat");
        for (id, timestamp) in [("A", 100), ("B", 200), ("C", 300)] {
            worker
                .archive
                .insert_message(&row(PEER, id, timestamp, id), None)
                .expect("stored");
        }

        let page = ask(
            &mut worker,
            "get_messages",
            serde_json::json!({ "chat": PEER, "limit": 2 }),
        )
        .expect("a page");
        let messages = page["messages"].as_array().expect("an array");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["id"], "B");
        assert_eq!(messages[1]["id"], "C");
        assert_eq!(messages[0]["text"], "B");

        // A full page hands back where to continue; the last page does not.
        let cursor = page["next_before"].clone();
        assert_eq!(cursor["id"], "B");
        let older = ask(
            &mut worker,
            "get_messages",
            serde_json::json!({ "chat": PEER, "limit": 2, "before": cursor }),
        )
        .expect("a page");
        let messages = older["messages"].as_array().expect("an array");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["id"], "A");
        assert!(older["next_before"].is_null(), "{older}");
    }

    #[test]
    fn a_search_result_names_the_chat_it_came_from() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker
            .archive
            .ensure_chat(PEER, "Ana Lopez")
            .expect("a chat");
        worker
            .archive
            .insert_message(&row(PEER, "A", 100, "the keys are under the mat"), None)
            .expect("stored");

        let found = ask(
            &mut worker,
            "search_messages",
            serde_json::json!({ "query": "keys" }),
        )
        .expect("results");
        let messages = found["messages"].as_array().expect("an array");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["chat_name"], "Ana Lopez");
        assert_eq!(messages[0]["chat"], PEER);
    }

    #[test]
    fn status_says_whether_sending_is_permitted_and_how_much_is_left() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "Ana").expect("a chat");

        let status = ask(&mut worker, "status", serde_json::json!({})).expect("a status");
        assert_eq!(status["can_send"], true);
        assert_eq!(status["sends_left"], crate::agent::SENDS_PER_WINDOW);
        assert_eq!(status["chats"], 1);
        assert_eq!(status["me"], ME);

        worker.agent_policy = Policy::new(true, false, true);
        let status = ask(&mut worker, "status", serde_json::json!({})).expect("a status");
        assert_eq!(status["can_send"], false);
    }

    /// The refusal must come before anything reaches WhatsApp, and before the
    /// send budget is spent on a request that was never going to go out.
    #[test]
    fn a_read_only_policy_refuses_a_send_without_spending_the_budget() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "Ana").expect("a chat");
        worker.agent_policy = Policy::new(true, false, true);

        let failure = ask(
            &mut worker,
            "send_text",
            serde_json::json!({ "chat": PEER, "text": "on my way" }),
        )
        .expect_err("a refusal");
        assert_eq!(failure.code(), "refused");
        assert_eq!(worker.agent_budget.left(), crate::agent::SENDS_PER_WINDOW);
    }

    /// Without a client there is nothing to hand the message to. Saying so
    /// beats archiving a message that will never leave the machine.
    #[test]
    fn a_send_with_no_phone_link_reports_itself_offline() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "Ana").expect("a chat");
        assert!(worker.client.is_none());

        let failure = ask(
            &mut worker,
            "send_text",
            serde_json::json!({ "chat": PEER, "text": "on my way" }),
        )
        .expect_err("a refusal");
        assert_eq!(failure.code(), "offline");
        assert_eq!(failure.status(), 503);
        // Nothing was archived, so the window shows no phantom message.
        assert!(
            worker
                .archive
                .messages(PEER, None, 10)
                .expect("a page")
                .is_empty()
        );
    }

    /// `send_text` drops a quote it cannot find, which would silently turn a
    /// reply into a loose message.
    #[test]
    fn a_quote_that_is_not_in_the_archive_is_refused() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "Ana").expect("a chat");

        let failure = ask(
            &mut worker,
            "send_text",
            serde_json::json!({ "chat": PEER, "text": "yes", "quoting": "NOPE" }),
        )
        .expect_err("a refusal");
        assert_eq!(failure.code(), "not_found");
        assert!(failure.message().contains("quoted message"), "{failure}");
    }

    #[test]
    fn an_announcement_group_refuses_a_send_rather_than_failing_at_the_server() {
        let (mut worker, _events, _inbox, _wa) = worker();
        let group = "1-2@g.us";
        worker
            .archive
            .ensure_chat(group, "Notices")
            .expect("a chat");
        worker
            .archive
            .set_group_info(group, None, &[ME.to_owned()], true)
            .expect("group info");

        let failure = ask(
            &mut worker,
            "send_text",
            serde_json::json!({ "chat": group, "text": "hello" }),
        )
        .expect_err("a refusal");
        assert_eq!(failure.code(), "refused");
        assert!(failure.message().contains("announcement"), "{failure}");
    }

    #[test]
    fn marking_a_chat_read_clears_its_unread_count() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "Ana").expect("a chat");
        worker.archive.set_unread(PEER, 4).expect("unread");

        let answer = ask(
            &mut worker,
            "mark_read",
            serde_json::json!({ "chat": PEER }),
        )
        .expect("an answer");
        assert_eq!(answer["receipts"], true);
        let chat = worker.archive.chat(PEER).expect("a chat").expect("a row");
        assert_eq!(chat.unread, 0);
    }

    #[test]
    fn a_group_chat_lists_its_members_with_names() {
        let (mut worker, _events, _inbox, _wa) = worker();
        let group = "1-2@g.us";
        worker
            .archive
            .ensure_chat(group, "Builders")
            .expect("a chat");
        worker
            .archive
            .set_group_info(group, None, &[ME.to_owned(), PEER.to_owned()], false)
            .expect("group info");

        let answer = ask(
            &mut worker,
            "get_chat",
            serde_json::json!({ "chat": group }),
        )
        .expect("a chat");
        let members = answer["members"].as_array().expect("an array");
        assert_eq!(members.len(), 2);
        let ids: Vec<_> = members.iter().map(|member| member["id"].clone()).collect();
        assert!(ids.contains(&serde_json::json!(ME)), "{members:?}");
        assert!(ids.contains(&serde_json::json!(PEER)), "{members:?}");
    }
}
