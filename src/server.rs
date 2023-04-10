use atc::{AiTcpCommand, create_listener};
use log::{error, info, warn};
use std::{collections::BTreeMap, error::Error, path::PathBuf, time::Duration};
use tokio::sync::mpsc::{channel, Sender};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if PathBuf::from("log4rs.yml").exists() {
        log4rs::init_file("log4rs.yml", Default::default()).unwrap();
    }

    let mut socket_map = BTreeMap::<String, Sender<AiTcpCommand>>::new();

    let (tx_up, mut rx_up) = channel::<(String, Option<Sender<AiTcpCommand>>)>(1024);

    tokio::spawn(async move {
        loop {
            if let Ok((job_id, sender)) = rx_up.try_recv() {
                match sender {
                    None => {
                        println!("Removing {} from socket map", job_id);
                        socket_map.remove(&job_id);
                    }
                    Some(sender) => {
                        println!("Adding {} to socket map", job_id);
                        socket_map.insert(job_id.clone(), sender);
                    }
                };
            }

            for (job_id, sender) in socket_map.iter() {
                let job_id = job_id.clone();
                let sender = sender.clone();

                if let Err(e) = sender
                    .send(AiTcpCommand::ChannelMessage((
                        job_id.clone(),
                        format!("Message from controller to job `{}`: Hello!", job_id),
                    )))
                    .await
                {
                    warn!(target: "atc-listener", "Error sending to message channel: {:?}", e);
                } else {
                    println!("Message sent");
                };
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });

    info!(target: "atc-listener", "Ready to start server.");
    let uri = "0.0.0.0:52926".to_string();
    if let Err(e) = create_listener(uri.clone(), tx_up).await {
        error!(target: "atc-listener", "Unable to bind to `{}`: `{:?}`", uri, e );
    }

    Ok(())
}
