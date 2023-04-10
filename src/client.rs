use atc::{create_connector, AiTcpCommand};
use log::error;
use std::{error::Error, path::PathBuf, time::Instant};
use tokio::sync::mpsc::channel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if PathBuf::from("log4rs.yml").exists() {
        log4rs::init_file("log4rs.yml", Default::default()).unwrap();
    }

    let (tx_ctrl, rx_ctrl) = channel::<AiTcpCommand>(1);
    let (tx_msg, mut rx_msg) = channel::<AiTcpCommand>(1);

    let id = uuid::Uuid::new_v4().to_string();

    // Create another async task that always execute.
    // Move the message channel receiver and move a clone of control channel
    // sender into the task
    let tx_ctrl = tx_ctrl.clone();
    tokio::spawn(async move {
        let mut last_ping = Instant::now();

        loop {
            if last_ping.elapsed().as_secs() >= 5 {
                match tx_ctrl.send(AiTcpCommand::Ping).await {
                    Ok(_) => {
                        last_ping = Instant::now();
                    }
                    Err(e) => {
                        error!(target: "atc-connector", "Unable to initualize PING command: {:?}", e);
                        return;
                    }
                };
            }
            if let Ok(data) = rx_msg.try_recv() {
                println!("Command: {:?}", data);
            };
        }
    });

    if let Err(e) = create_connector("127.0.0.1:52926".into(), id, rx_ctrl, tx_msg.clone()).await {
        error!(target: "atc-connector", "Unable to connect to remote server: {:?}", e);
    }

    Ok(())
}
