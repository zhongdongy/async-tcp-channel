use lazy_static::lazy_static;
use regex::Regex;

use super::frame::Frame;

lazy_static! {
    static ref RE_CMD_IDENTIFY: Regex = Regex::new(r"^@identify<(?P<job_id>[a-f\d-]+)>").unwrap();
    static ref RE_CMD_CHANNEL_MESSAGE: Regex =
        Regex::new(r"^@message<(?P<job_id>[a-f\d-]+)>(?P<message>.*)$").unwrap();
    static ref RE_CMD_TERMINATE: Regex = Regex::new(r"^@terminate<(?P<job_id>[a-f\d-]+)>").unwrap();
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerCommand {
    /// Send message to give target.
    Message(Option<String>, String),

    /// Terminate server.
    Terminate,
}

#[derive(Debug, Clone)]
pub enum ChannelCommand {
    /// Identify the connection endpoint, normally use `job-id`.
    Identify(String),

    /// Terminate connection.
    Terminate(String),

    /// Channel message. normally sent by server.
    ChannelMessage((String, String)),

    /// Ping msg
    Ping,

    /// Pong msg
    Pong,
}

impl Into<Frame> for ChannelCommand {
    fn into(self) -> Frame {
        (match self {
            ChannelCommand::Identify(id) => format!("@identify<{}>", id),
            ChannelCommand::Terminate(id) => format!("@terminate<{}>", id),
            ChannelCommand::ChannelMessage((id, msg)) => {
                format!("@message<{}>{}", id, msg)
            }
            ChannelCommand::Ping => format!("@ping"),
            ChannelCommand::Pong => format!("@pong"),
        })
        .into()
    }
}

impl From<Frame> for ChannelCommand {
    fn from(value: Frame) -> Self {
        let value: String = value.into();
        if &value == "@ping" {
            return Self::Ping;
        } else if &value == "@pong" {
            return Self::Pong;
        }

        if RE_CMD_IDENTIFY.is_match(&value) {
            if let Some(cap) = RE_CMD_IDENTIFY.captures(&value) {
                if let Some(mat) = cap.name("job_id") {
                    return Self::Identify(mat.as_str().to_string());
                }
            }
        } else if RE_CMD_TERMINATE.is_match(&value) {
            if let Some(cap) = RE_CMD_TERMINATE.captures(&value) {
                if let Some(mat) = cap.name("job_id") {
                    return Self::Terminate(mat.as_str().to_string());
                }
            }
        } else if RE_CMD_CHANNEL_MESSAGE.is_match(&value) {
            if let Some(cap) = RE_CMD_CHANNEL_MESSAGE.captures(&value) {
                if let Some(mat_id) = cap.name("job_id") {
                    if let Some(mat_msg) = cap.name("message") {
                        return Self::ChannelMessage((
                            mat_id.as_str().to_string(),
                            mat_msg.as_str().to_string(),
                        ));
                    }
                    return Self::ChannelMessage((mat_id.as_str().to_string(), String::new()));
                }
            }
        }
        panic!("Unknown AiTcpCommand input: `{}`", value);
    }
}
