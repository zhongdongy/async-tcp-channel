use std::{error::Error, path::PathBuf, time::Duration};

use atc::{Server, ServerCommand};
use tokio::{sync::mpsc::channel, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if PathBuf::from("log4rs.yml").exists() {
        log4rs::init_file("log4rs.yml", Default::default()).unwrap();
    }

    let (tx, rx) = channel::<ServerCommand>(1024);

    tokio::spawn(async move {
        let timer = Instant::now();
        loop {
            if timer.elapsed().as_secs() >= 5 {
                tx.send(ServerCommand::Terminate).await.unwrap();
                break;
            }

            tx.send(ServerCommand::Message(
                None,
                format!("{} seconds to explode!", { 5 - timer.elapsed().as_secs() }),
            ))
            .await
            .unwrap();
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    let mut server = Server::new("0.0.0.0:52926".into(), rx);
    server.start().await.unwrap();

    println!("Server now terminated");
    Ok(())
}
