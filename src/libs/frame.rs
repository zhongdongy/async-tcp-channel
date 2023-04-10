use std::{error::Error, fmt::Display};

use log::{debug, warn};

#[derive(Debug, Clone)]
pub struct Frame {
    content: Vec<u8>,
}

impl Frame {
    pub fn new(content: Vec<u8>) -> Self {
        Self { content }
    }

    pub fn parse_sequence(seq: &[u8], existing: Option<Vec<u8>>) -> (Vec<Frame>, Vec<u8>) {
        let mut parsed = vec![];

        let mut byte_cache: Vec<u8> = vec![];
        let mut temp: Vec<u8> = vec![];
        if let Some(existing) = existing {
            existing.iter().for_each(|u| {
                temp.push(*u);
            });
        }
        for ch in seq {
            if *ch == 0x7e {
                if temp.len() > 0 {
                    // Submit
                    if let Ok(frame) = Self::parse(&temp) {
                        debug!(target: "atc-frame", "Parsed frame: `{}`", frame);
                        parsed.push(frame);
                    } else {
                        warn!(target: "atc-frame", "Unable to parse sequence: `{:?}`", temp);
                    }
                    temp.clear()
                }
            } else if *ch == 0x7d {
                // Put to byte cache
                byte_cache.push(0x7d);
            } else if *ch == 0x5d {
                if let Some(byte) = byte_cache.last() {
                    if *byte == 0x7d {
                        temp.push(0x7d);
                        byte_cache.clear();
                    }
                }
            } else if *ch == 0x5e {
                if let Some(byte) = byte_cache.last() {
                    if *byte == 0x7d {
                        temp.push(0x7e);
                        byte_cache.clear();
                    }
                }
            } else {
                temp.push(*ch);
            }
        }
        if temp.len() > 0 {
            if let Ok(frame) = Frame::parse(&temp) {
                debug!(target: "atc-frame", "Parsed frame: `{}`", frame);
                parsed.push(frame);
                temp.clear();
            }
        }
        (parsed, temp.iter().map(|c| *c as u8).collect::<Vec<u8>>())
    }

    fn parse(temp: &Vec<u8>) -> Result<Frame, Box<dyn Error>> {
        Ok(Frame::new(temp.clone()))
    }
}

impl From<String> for Frame {
    fn from(value: String) -> Self {
        Self {
            content: value.as_bytes().to_vec(),
        }
    }
}

impl From<&str> for Frame {
    fn from(value: &str) -> Self {
        Self {
            content: value.as_bytes().to_vec(),
        }
    }
}

impl Display for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Frame<{}>",
            String::from_utf8(self.content.clone()).unwrap()
        )
    }
}

impl Into<Vec<u8>> for Frame {
    fn into(self) -> Vec<u8> {
        let mut ret: Vec<u8> = vec![0x7e];
        self.content.iter().for_each(|u| {
            if *u == 0x7e {
                ret.push(0x7d);
                ret.push(0x5e);
            } else if *u == 0x7d {
                ret.push(0x7d);
                ret.push(0x5d);
            } else {
                ret.push(*u)
            }
        });

        ret
    }
}

impl Into<String> for Frame {
    fn into(self) -> String {
        String::from_utf8(self.content).unwrap()
    }
}
