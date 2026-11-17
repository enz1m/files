// What GIO asks while mounting: share credentials and questions, such as an
// unknown SFTP host key, that only the user can answer.

#[cfg(not(test))]
use super::OperationCancel;
#[cfg(not(test))]
use super::mount::MountCredentials;
use std::sync::Arc;

// Answers GIO's `ask-password` for one authenticated mount attempt.
//
// The callback answers once; a second prompt means refusal, so it clears the
// password and replies `Aborted`. It fills only the requested fields, saves the
// password permanently only when asked and supported, and aborts `TCRYPT`
// hidden-volume prompts. The password is only lent to `set_password`, never
// formatted or copied; dropping the operation drops the credentials. The
// returned counter counts prompts.
#[cfg(not(test))]
pub(crate) fn configure_ask_password_callback(
    mount_op: &gio::MountOperation,
    credentials: MountCredentials,
) -> Arc<std::sync::atomic::AtomicUsize> {
    use gio::prelude::MountOperationExt;
    use gio::{AskPasswordFlags, MountOperationResult, PasswordSave};
    use std::rc::Rc;
    use std::sync::atomic::Ordering;

    let call_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_cb = Arc::clone(&call_counter);
    let state: Rc<MountCredentials> = Rc::new(credentials);

    mount_op.connect_ask_password(move |op, _message, _default_user, _default_domain, flags| {
        let n = counter_cb.fetch_add(1, Ordering::SeqCst);

        if n > 0 {
            op.set_password(None);
            op.reply(MountOperationResult::Aborted);
            return;
        }

        if flags.contains(AskPasswordFlags::TCRYPT) {
            op.reply(MountOperationResult::Aborted);
            return;
        }

        if flags.contains(AskPasswordFlags::NEED_USERNAME) {
            op.set_username(Some(state.username()));
        }
        if flags.contains(AskPasswordFlags::NEED_DOMAIN) {
            op.set_domain(Some(state.domain()));
        }
        if flags.contains(AskPasswordFlags::NEED_PASSWORD) {
            op.set_password(Some(state.expose_password()));
        }

        op.set_password_save(
            if state.remembers_password() && flags.contains(AskPasswordFlags::SAVING_SUPPORTED) {
                PasswordSave::Permanently
            } else {
                PasswordSave::Never
            },
        );

        op.reply(MountOperationResult::Handled);
    });

    call_counter
}

// Characters a question's text may carry, after control characters are
// removed (newlines are kept).
const QUESTION_TEXT_LIMIT: usize = 1024;
// Characters one choice label may carry.
const QUESTION_CHOICE_LIMIT: usize = 64;
// Choices one question may offer; a longer list is declined unanswered.
const QUESTION_CHOICES_MAX: usize = 4;

#[cfg(not(test))]
const QUESTION_POLL_MS: u64 = 50;

// A question GVfs asks while mounting, such as an SFTP first-contact host key.
//
// The backend's text, bounded and stripped of control characters other than
// newlines, so it can be shown as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MountQuestion {
    pub(crate) message: String,
    pub(crate) choices: Vec<String>,
    // A question where going ahead may be dangerous (a server whose key
    // changed): presented as a warning, its choices as destructive.
    pub(crate) warning: bool,
}

impl MountQuestion {
    pub(crate) fn sanitized(message: &str, choices: &[String]) -> Option<Self> {
        let clean = |text: &str, limit: usize| -> String {
            text.chars()
                .filter(|c| *c == '\n' || !c.is_control())
                .take(limit)
                .collect::<String>()
                .trim()
                .to_owned()
        };
        let choices: Vec<String> = choices
            .iter()
            .map(|choice| clean(choice, QUESTION_CHOICE_LIMIT))
            .collect();
        if choices.is_empty()
            || choices.len() > QUESTION_CHOICES_MAX
            || choices.iter().any(String::is_empty)
        {
            return None;
        }
        Some(Self {
            message: clean(message, QUESTION_TEXT_LIMIT),
            choices,
            warning: false,
        })
    }

    pub(crate) fn warning(mut self) -> Self {
        self.warning = true;
        self
    }
}

// The user's answer to a [`MountQuestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MountAnswer {
    // The choice at this index.
    Choice(usize),
    // No choice: the dialog was dismissed, or the mount ended first.
    Decline,
}

// One question on its way to the user, with the slot its answer goes to.
// `Clone` so it can ride a `Msg`; only the first answer is delivered.
#[derive(Clone)]
pub(crate) struct AskedQuestion {
    pub(crate) question: MountQuestion,
    reply: Arc<std::sync::Mutex<Option<Reply>>>,
}

enum Reply {
    // A GIO mount waiting on its private context.
    #[cfg(not(test))]
    Blocking(std::sync::mpsc::SyncSender<MountAnswer>),
    // A future, such as a mount service conversation.
    Future(iced::futures::channel::oneshot::Sender<MountAnswer>),
}

impl AskedQuestion {
    // A question whose answer resolves the returned future; dropping the
    // question unanswered resolves it to [`MountAnswer::Decline`].
    pub(crate) fn asking(
        question: MountQuestion,
    ) -> (Self, impl std::future::Future<Output = MountAnswer>) {
        let (reply, answer) = iced::futures::channel::oneshot::channel();
        let asked = Self {
            question,
            reply: Arc::new(std::sync::Mutex::new(Some(Reply::Future(reply)))),
        };
        (asked, async move {
            answer.await.unwrap_or(MountAnswer::Decline)
        })
    }

    // Deliver `answer`. Later answers are ignored.
    pub(crate) fn answer(&self, answer: MountAnswer) {
        let reply = self
            .reply
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match reply {
            #[cfg(not(test))]
            Some(Reply::Blocking(reply)) => {
                let _ = reply.send(answer);
            }
            Some(Reply::Future(reply)) => {
                let _ = reply.send(answer);
            }
            None => {}
        }
    }
}

impl std::fmt::Debug for AskedQuestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskedQuestion")
            .field("question", &self.question)
            .finish_non_exhaustive()
    }
}

// Where a mount posts its questions. Holds at most one unanswered question.
#[derive(Clone, Debug)]
pub(crate) struct QuestionPort(
    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "The native GIO callback sends questions through this port"
        )
    )]
    iced::futures::channel::mpsc::Sender<AskedQuestion>,
);

pub(crate) fn question_channel() -> (
    QuestionPort,
    iced::futures::channel::mpsc::Receiver<AskedQuestion>,
) {
    let (sender, receiver) = iced::futures::channel::mpsc::channel(0);
    (QuestionPort(sender), receiver)
}

// Answers GVfs's `ask-question` through the user.
//
// The operation waits on the private context for the answer sent to `port`. It is
// declined, never answered for the user, when there is no port, the question is
// malformed or has more than [`QUESTION_CHOICES_MAX`] choices, the port is busy
// or gone, the answer is out of range, or the mount is cancelled. The mount's
// inner deadline bounds the wait.
#[cfg(not(test))]
pub(crate) fn configure_ask_question_callback(
    mount_op: &gio::MountOperation,
    port: Option<QuestionPort>,
    op_cancel: OperationCancel,
) {
    use gio::MountOperationResult;
    use gio::prelude::MountOperationExt;
    use glib::prelude::ObjectExt;

    mount_op.connect_local("ask-question", false, move |values| {
        let op = values.first()?.get::<gio::MountOperation>().ok()?;
        let message = values
            .get(1)
            .and_then(|value| value.get::<String>().ok())
            .unwrap_or_default();
        let choices = values
            .get(2)
            .and_then(|value| value.get::<Vec<String>>().ok())
            .unwrap_or_default();
        let question = MountQuestion::sanitized(&message, &choices);
        let (Some(port), Some(question)) = (port.clone(), question) else {
            op.reply(MountOperationResult::Aborted);
            return None;
        };
        let count = question.choices.len();
        let (reply, answers) = std::sync::mpsc::sync_channel(1);
        let asked = AskedQuestion {
            question,
            reply: Arc::new(std::sync::Mutex::new(Some(Reply::Blocking(reply)))),
        };
        let mut sender = port.0;
        if sender.try_send(asked).is_err() {
            op.reply(MountOperationResult::Aborted);
            return None;
        }
        let cancel = op_cancel.clone();
        // On the mount's private context, which is the thread-default one
        // while the handler runs.
        glib::MainContext::ref_thread_default().spawn_local(async move {
            use std::sync::mpsc::TryRecvError;
            let answer = loop {
                match answers.try_recv() {
                    Ok(answer) => break answer,
                    Err(TryRecvError::Empty) if !cancel.is_cancelled() => {}
                    Err(_) => break MountAnswer::Decline,
                }
                glib::timeout_future(std::time::Duration::from_millis(QUESTION_POLL_MS)).await;
            };
            match answer {
                MountAnswer::Choice(index) if index < count => {
                    op.set_choice(i32::try_from(index).unwrap_or(-1));
                    op.reply(MountOperationResult::Handled);
                }
                _ => op.reply(MountOperationResult::Aborted),
            }
        });
        None
    });
}
