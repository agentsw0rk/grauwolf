//! History of the run and the attachments waiting for the next user message (ADR-0081).

use grauwolf_prompt::{Attachment, Message, Prompt};

/// The history (`Prompt`) and the attachments of the run's input.
pub(crate) struct Conversation {
    /// History so far.
    prompt: Prompt,
    /// Attachments for the next user message, i.e. the input of the run (ADR-0081).
    pending_attachments: Vec<Attachment>,
}

impl Conversation {
    /// Conversation starting with `prompt` and no pending attachments.
    pub(crate) fn new(prompt: Prompt) -> Self {
        Self {
            prompt,
            pending_attachments: Vec::new(),
        }
    }

    /// Conversation of a fork: copies of history and pending attachments.
    pub(crate) fn fork(&self) -> Self {
        Self {
            prompt: self.prompt.clone(),
            pending_attachments: self.pending_attachments.clone(),
        }
    }

    /// History so far.
    pub(crate) fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// The history for a caller that changes it in place (compression, tool choice).
    pub(crate) fn prompt_mut(&mut self) -> &mut Prompt {
        &mut self.prompt
    }

    /// The history, given up.
    pub(crate) fn into_prompt(self) -> Prompt {
        self.prompt
    }

    /// Replaces the history with `prompt` and returns the previous one.
    pub(crate) fn replace_prompt(&mut self, prompt: Prompt) -> Prompt {
        std::mem::replace(&mut self.prompt, prompt)
    }

    /// Appends `message` to the history; a user message without attachments takes the pending ones.
    pub(crate) fn append(&mut self, mut message: Message) {
        if let Message::User { attachments, .. } = &mut message
            && attachments.is_empty()
        {
            *attachments = std::mem::take(&mut self.pending_attachments);
        }
        self.prompt.messages.push(message);
    }

    /// Attaches `attachments` to the next user message (ADR-0081).
    pub(crate) fn attach_to_input(&mut self, attachments: Vec<Attachment>) {
        self.pending_attachments = attachments;
    }
}

#[cfg(test)]
mod tests {
    use grauwolf_prompt::AttachmentData;

    use super::*;

    fn image() -> Attachment {
        Attachment {
            media_type: "image/png".into(),
            data: AttachmentData::Url("https://example.test/a.png".into()),
        }
    }

    fn attachments_of(message: &Message) -> &[Attachment] {
        match message {
            Message::User { attachments, .. } => attachments,
            other => panic!("not a user message: {other:?}"),
        }
    }

    #[test]
    fn append_adds_messages_in_order() {
        let mut conversation = Conversation::new(Prompt::builder("p").system("rules").build());
        conversation.append(Message::user("hi"));
        conversation.append(Message::assistant("hello"));
        assert_eq!(conversation.prompt().messages.len(), 3);
        assert_eq!(conversation.prompt().messages[1], Message::user("hi"));
    }

    #[test]
    fn pending_attachments_go_to_the_next_user_message_once() {
        let mut conversation = Conversation::new(Prompt::builder("p").build());
        conversation.attach_to_input(vec![image()]);
        conversation.append(Message::user("look"));
        conversation.append(Message::user("again"));
        let messages = &conversation.prompt().messages;
        assert_eq!(attachments_of(&messages[0]), [image()]);
        assert!(attachments_of(&messages[1]).is_empty());
    }

    #[test]
    fn pending_attachments_wait_for_a_user_message() {
        let mut conversation = Conversation::new(Prompt::builder("p").build());
        conversation.attach_to_input(vec![image()]);
        conversation.append(Message::assistant("no user yet"));
        conversation.append(Message::user("now"));
        assert_eq!(
            attachments_of(&conversation.prompt().messages[1]),
            [image()]
        );
    }

    #[test]
    fn own_attachments_of_a_message_stay_and_pending_ones_remain() {
        let mut conversation = Conversation::new(Prompt::builder("p").build());
        conversation.attach_to_input(vec![image()]);
        let own = Attachment {
            media_type: "application/pdf".into(),
            data: AttachmentData::Base64("AAAA".into()),
        };
        conversation.append(Message::user_with("own", vec![own.clone()]));
        conversation.append(Message::user("next"));
        let messages = &conversation.prompt().messages;
        assert_eq!(attachments_of(&messages[0]), [own]);
        assert_eq!(attachments_of(&messages[1]), [image()]);
    }

    #[test]
    fn replace_prompt_returns_the_previous_history() {
        let mut conversation = Conversation::new(Prompt::builder("old").user("a").build());
        let previous = conversation.replace_prompt(Prompt::builder("new").build());
        assert_eq!(previous.id, "old");
        assert_eq!(conversation.prompt().id, "new");
        assert!(conversation.prompt().messages.is_empty());
    }

    #[test]
    fn fork_copies_history_and_pending_attachments() {
        let mut conversation = Conversation::new(Prompt::builder("p").build());
        conversation.attach_to_input(vec![image()]);
        let mut fork = conversation.fork();
        fork.append(Message::user("in the branch"));
        assert!(conversation.prompt().messages.is_empty());
        assert_eq!(attachments_of(&fork.prompt().messages[0]), [image()]);
        // The original still holds its own copy of the attachments.
        conversation.append(Message::user("in the trunk"));
        assert_eq!(
            attachments_of(&conversation.prompt().messages[0]),
            [image()]
        );
    }
}
