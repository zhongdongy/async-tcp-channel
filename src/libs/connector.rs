use crate::{ChannelCommand, Frame};

use log::{debug, error, info, warn};
use std::{io::ErrorKind, sync::Arc, time::Duration};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{
        mpsc::{error::TryRecvError, Receiver, Sender},
        Mutex,
    },
    time::timeout,
};

/// Connect to remote endpoint with given command receiver as controller and
/// a sender as remote message proxy.
pub async fn create_connector(
    uri: String,
    identity: String,
    rx_ctrl: Arc<Mutex<Receiver<ChannelCommand>>>,
    tx_msg: Sender<ChannelCommand>,
    flag_int: Arc<Mutex<bool>>,
) -> io::Result<()> {
    let mut stream = TcpStream::connect(uri.clone()).await?;

    let initial_command = ChannelCommand::Identify(identity.clone());

    let mut command = Some(initial_command);
    let mut buffer = vec![0; 1024];
    let mut read_timeout_ms = 16f32;
    let mut parsing_buffer: Vec<u8> = vec![];
    loop {
        {
            if *flag_int.lock().await == true {
                break Ok(());
            }
        }
        if !command.is_none() {
            let command_clone = command.clone().unwrap();
            let frame: Frame = Into::<Frame>::into(command_clone);
            let frame_bytes: Vec<u8> = frame.clone().into();
            match stream.write(&frame_bytes).await {
                Ok(_) => {
                    debug!(target: "atc-connector", "Message sent to remote endpoint: {}", frame);
                    command = None;
                }
                Err(e) => {
                    if e.kind() == ErrorKind::ConnectionReset
                        || e.kind() == ErrorKind::ConnectionAborted
                    {
                        error!(target: "atc-connector", "Connection to remote endpoint broken, message cannot be sent: {:?}", e);
                        break Ok(());
                    } else {
                        warn!(target: "atc-connector", "Error writing message to TCP socket: {:?}", e);
                    }
                }
            };
        }

        {
            match rx_ctrl.lock().await.try_recv() {
                Ok(cmd) => {
                    command = match cmd {
                        ChannelCommand::Terminate(_) => {
                            info!(target: "atc-connector", "User requested job termination, current job will be discarded.");
                            Some(cmd)
                        }
                        ChannelCommand::Identify(_) => {
                            info!(target: "atc-connector", "User re-identification.");
                            Some(cmd)
                        }
                        ChannelCommand::Ping => Some(cmd),
                        _ => {
                            warn!(target: "atc-connector", "User requested to send command other than `[ChannelMessage]`, `[Identify]` and `[Terminate]`.");
                            panic!(
                                "You should ONLY send `[ChannelMessage]`, `[Identify]` or `[Terminate]` command."
                            );
                        }
                    };
                }
                Err(e) => {
                    if e == TryRecvError::Disconnected {
                        error!(target: "atc-connector", "Command channel receiver disconnected: {:?}", e);
                        break Ok(());
                    }
                }
            }
        }

        if let Ok(res) = timeout(
            Duration::from_millis(read_timeout_ms.floor() as u64),
            stream.read(&mut buffer),
        )
        .await
        {
            match res {
                Ok(n) => {
                    if n > 0 {
                        // Reset read timeout
                        read_timeout_ms = 16f32;

                        let (frames, remain) =
                            Frame::parse_sequence(&buffer[0..n], Some(parsing_buffer));
                        parsing_buffer = remain;
                        buffer = vec![0; 1024];

                        // let command =
                        // AiTcpCommand::from(String::from_utf8(buffer[0..n].to_vec()).unwrap());
                        for frame in frames {
                            let command: ChannelCommand = frame.into();
                            if let ChannelCommand::ChannelMessage((id, _)) = command.clone() {
                                if id == identity {
                                    tx_msg.send(command).await.unwrap();
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if e.kind() == ErrorKind::ConnectionReset
                        || e.kind() == ErrorKind::ConnectionAborted
                    {
                        error!(target: "atc-connector", "Error reading from TCP socket, possible because of server unavailable or broken: {:?}", e);
                        break Ok(());
                    }
                    warn!(target: "atc-connector", "Error reading message from TCP socket: {:?}", e);
                }
            }
        } else {
            // Should increase
            read_timeout_ms *= 1.25;
            read_timeout_ms = read_timeout_ms.min(4096.0);
        }
    }
}
