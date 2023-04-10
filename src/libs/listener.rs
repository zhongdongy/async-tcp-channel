use crate::{ChannelCommand, Frame};

use log::{debug, error, info, warn};
use std::{io::ErrorKind, sync::Arc, time::Duration};
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{
        mpsc::{channel, Sender},
        Mutex,
    },
    time::timeout,
};

/// Create a TCP listener and a upstream channel sender receiver
/// to allow stream/socket identification and registration.
pub async fn create_listener(
    uri: String,
    tx_up: Sender<(String, Option<Sender<ChannelCommand>>)>,
    flag_int: Arc<Mutex<bool>>,
) -> io::Result<()> {
    let listener = TcpListener::bind(uri.clone()).await?;

    // Listener loop. For ever incoming socket/stream, create a new async task
    // to handle it.
    loop {
        {
            if *flag_int.lock().await == true {
                return Ok(());
            }
        }

        if let Ok(listen_result) = timeout(Duration::from_secs(5), listener.accept()).await {
            if let Ok((mut stream, _)) = listen_result {
                let tx_up = tx_up.clone();

                let flag_int_clone = flag_int.clone();
                tokio::spawn(async move {
                    info!(target: "atc-listener", "New incomming connection");
                    let (tx_down, mut rx_down) = channel::<ChannelCommand>(1024);
                    let mut buffer = vec![0u8; 1024];
                    let mut parsing_buffer: Vec<u8> = vec![];
                    let mut write_command_buffer: Option<ChannelCommand> = None;

                    let mut job_id = String::new();
                    let mut read_timeout_ms = 16f32;

                    loop {
                        {
                            if *flag_int_clone.lock().await == true {
                                return;
                            }
                        }
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
                            Duration::from_millis(read_timeout_ms.floor() as u64),
                            stream.read(&mut buffer),
                        )
                        .await
                        {
                            {
                                if *flag_int_clone.lock().await == true {
                                    return;
                                }
                            }
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
                                    read_timeout_ms = 16f32;

                                    let (frames, remain) =
                                        Frame::parse_sequence(&buffer[0..n], Some(parsing_buffer));
                                    parsing_buffer = remain;
                                    // let content = String::from_utf8(buffer[0..n].to_vec()).unwrap();

                                    // debug!(target: "atc-listener", "RAW MESSAGE: `{}`", content);

                                    buffer = vec![0u8; 1024];

                                    for frame in frames {
                                        let command = ChannelCommand::from(frame);

                                        match command {
                                            ChannelCommand::Identify(id) => {
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
                                            ChannelCommand::Terminate(job_id) => {
                                                tx_up.send((job_id.clone(), None)).await.unwrap();
                                            }
                                            ChannelCommand::Ping => {
                                                debug!(target: "atc-listener", "Ping received from `{}`", job_id);
                                                write_command_buffer = Some(ChannelCommand::Pong);
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
                            read_timeout_ms *= 1.25;
                            read_timeout_ms = read_timeout_ms.min(4096.0);
                        }
                    }

                    info!(target: "atc-listener", "Socket/stream handler for job-id=`{}` closed.", job_id);
                });
            }
        } else {
            debug!(target: "atc-listener", "No new connections in last 5 seconds");
        }
    }
}
