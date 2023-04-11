use atc::{Client, ServerCommand};
use std::{error::Error, path::PathBuf, time::Duration};
use tokio::sync::mpsc::channel;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if PathBuf::from("log4rs.yml").exists() {
        log4rs::init_file("log4rs.yml", Default::default()).unwrap();
    }

    let (tx, rx) = channel::<ServerCommand>(1024);

    tokio::spawn(async move {
        loop {
            std::thread::sleep(Duration::from_secs(5));
            tx.send(ServerCommand::Identify(uuid::Uuid::new_v4().to_string()))
                .await
                .unwrap();
        }
    });

    let mut client = Client::new(
        "127.0.0.1:52926".into(),
        "test-async-tcp-channel".into(),
        rx,
    )
    .reconnect(true)
    .callback(|job_id, msg| {
        println!("Message from `{}`: {}", job_id, msg);
    })
    .await;

    client.connect().await.unwrap();

    Ok(())
}
