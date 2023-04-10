pub mod libs;

pub use libs::{command::AiTcpCommand, frame::Frame};

use log::{debug, error, info, warn};
use std::{io::ErrorKind, time::Duration};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc::{channel, error::TryRecvError, Receiver, Sender},
    time::timeout,
};

/// Connect to remote endpoint with given command receiver as controller and
/// a sender as remote message proxy.
pub async fn create_connector(
    uri: String,
    identity: String,
    mut rx_ctrl: Receiver<AiTcpCommand>,
    tx_msg: Sender<AiTcpCommand>,
) -> io::Result<()> {
    let mut stream = TcpStream::connect(uri.clone()).await?;

    let initial_command = AiTcpCommand::Identify(identity.clone());

    let mut command = Some(initial_command);
    let mut buffer = vec![0; 1024];
    let mut read_timeout_ms = 16u64;
    let mut parsing_buffer: Vec<u8> = vec![];
    loop {
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

        match rx_ctrl.try_recv() {
            Ok(cmd) => {
                command = match cmd {
                    AiTcpCommand::Terminate(_) => {
                        info!(target: "atc-connector", "User requested job termination, current job will be discarded.");
                        Some(cmd)
                    }
                    AiTcpCommand::Ping => Some(cmd),
                    _ => {
                        warn!(target: "atc-connector", "User requested to send command other than `[ChannelMessage]` and `[Terminate]`.");
                        panic!("You should ONLY send `[ChannelMessage]` or `[Terminate]` command.");
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

        if let Ok(res) = timeout(
            Duration::from_millis(read_timeout_ms),
            stream.read(&mut buffer),
        )
        .await
        {
            match res {
                Ok(n) => {
                    if n > 0 {
                        // Reset read timeout
                        read_timeout_ms = 16;

                        let (frames, remain) =
                            Frame::parse_sequence(&buffer[0..n], Some(parsing_buffer));
                        parsing_buffer = remain;
                        buffer = vec![0; 1024];

                        // let command =
                        // AiTcpCommand::from(String::from_utf8(buffer[0..n].to_vec()).unwrap());
                        for frame in frames {
                            let command: AiTcpCommand = frame.into();
                            if let AiTcpCommand::ChannelMessage((id, _)) = command.clone() {
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
            read_timeout_ms *= 2;
            read_timeout_ms = read_timeout_ms.min(4096);
        }
    }
}

/// Create a TCP listener and a upstream channel sender receiver
/// to allow stream/socket identification and registration.
pub async fn create_listener(
    uri: String,
    tx_up: Sender<(String, Option<Sender<AiTcpCommand>>)>,
) -> io::Result<()> {
    let listener = TcpListener::bind(uri.clone()).await?;

    // Listener loop. For ever incoming socket/stream, create a new async task
    // to handle it.
    loop {
        if let Ok((mut stream, _)) = listener.accept().await {
            let tx_up = tx_up.clone();
            tokio::spawn(async move {
                info!(target: "atc-listener", "New incomming connection");
                let (tx_down, mut rx_down) = channel::<AiTcpCommand>(1024);
                let mut buffer = vec![0u8; 1024];
                let mut parsing_buffer: Vec<u8> = vec![];
                let mut write_command_buffer: Option<AiTcpCommand> = None;

                let mut job_id = String::new();
                let mut read_timeout_ms = 16u64;

                loop {
                    match rx_down.try_recv() {
                        Ok(command) => {
                            write_command_buffer = Some(command);
                        }
                        Err(_) => (),
                    }

                    if let Some(command) = write_command_buffer.clone() {
                        let frame: Frame = command.into();
                        let frame_bytes: Vec<u8> = frame.into();
                        match stream.write_all(&frame_bytes).await {
                            Ok(_) => {
                                write_command_buffer = None;
                            }
                            Err(e) => {
                                if e.kind() == ErrorKind::ConnectionReset {
                                    warn!(target: "atc-listener", "Connection closed by peer.");
                                    break;
                                }
                                warn!(target:"atc-listener", "Unable to write to stream: {:?}", e);
                            }
                        };
                    }

                    if let Ok(res) = timeout(
                        Duration::from_millis(read_timeout_ms),
                        stream.read(&mut buffer),
                    )
                    .await
                    {
                        match res {
                            Ok(n) => {
                                if n == 0 {
                                    if let Err(e) = stream.shutdown().await {
                                        // Notify manager a connection is down and should be removed from btree-map.
                                        tx_up.send((job_id.clone(), None)).await.unwrap();
                                        warn!(target: "atc-listener", "Unable to shutdown TCP connection from server side: {:?}", e);
                                    };
                                    break (());
                                }
                                // Reset read timeout.
                                read_timeout_ms = 16;

                                let (frames, remain) =
                                    Frame::parse_sequence(&buffer[0..n], Some(parsing_buffer));
                                parsing_buffer = remain;
                                // let content = String::from_utf8(buffer[0..n].to_vec()).unwrap();

                                // debug!(target: "atc-listener", "RAW MESSAGE: `{}`", content);

                                buffer = vec![0u8; 1024];

                                for frame in frames {
                                    let command = AiTcpCommand::from(frame);

                                    match command {
                                        AiTcpCommand::Identify(id) => {
                                            job_id = id;
                                            info!(target: "atc-listener", "Received identification: {}", job_id);
                                            match tx_up
                                                .send((job_id.clone(), Some(tx_down.clone())))
                                                .await
                                            {
                                                Ok(_) => (),
                                                Err(_) => {
                                                    error!(target: "atc-listener", "Unable to write to server control channel sender.")
                                                }
                                            }
                                        }
                                        AiTcpCommand::Terminate(job_id) => {
                                            tx_up.send((job_id.clone(), None)).await.unwrap();
                                        }
                                        AiTcpCommand::Ping => {
                                            debug!(target: "atc-listener", "Ping received from `{}`", job_id);
                                            write_command_buffer = Some(AiTcpCommand::Pong);
                                        }
                                        _ => (),
                                    }
                                }
                            }

                            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => {
                                debug!(target: "atc-listener", "Connection broken [job-id={}]", job_id);
                                if let Err(e) = tx_up.send((job_id.clone(), None)).await {
                                    error!(target: "atc-listener", "Unable to send control message to channel [connection broken]: {:?}", e);
                                };
                                break;
                            }
                            Err(e) => {
                                error!(target: "atc-listener", "Error writing bytes to channel: {:?}", e);
                                break;
                            }
                        }
                    } else {
                        // Should increase
                        read_timeout_ms *= 2;
                        read_timeout_ms = read_timeout_ms.min(4096);
                    }
                }

                info!(target: "atc-listener", "Socket/stream handler for job-id=`{}` closed.", job_id);
            });
        }
    }
}
