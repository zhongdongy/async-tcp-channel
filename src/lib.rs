pub mod libs;

pub use libs::{
    command::{ChannelCommand, ServerCommand},
    connector::create_connector,
    frame::Frame,
    listener::create_listener,
};

use log::{debug, error, info, warn};
use std::{collections::BTreeMap, sync::Arc};
use tokio::{
    io,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
    time::Instant,
};

pub struct Server {
    socket_map: Arc<Mutex<BTreeMap<String, Sender<ChannelCommand>>>>,
    rx_upstream: Arc<Mutex<Receiver<(String, Option<Sender<ChannelCommand>>)>>>,
    tx_upstream: Sender<(String, Option<Sender<ChannelCommand>>)>,
    rx_control: Arc<Mutex<Receiver<ServerCommand>>>,
    uri: String,
    server_started: Arc<Mutex<bool>>,
    flag_interrupt: Arc<Mutex<bool>>,
}

impl Server {
    pub fn new(uri: String, rx_ctrl: Receiver<ServerCommand>) -> Self {
        let (tx, rx) = channel(1024);
        Self {
            uri,
            socket_map: Arc::new(Mutex::new(BTreeMap::<String, Sender<ChannelCommand>>::new())),
            rx_upstream: Arc::new(Mutex::new(rx)),
            tx_upstream: tx,
            server_started: Arc::new(Mutex::new(false)),
            flag_interrupt: Arc::new(Mutex::new(false)),
            rx_control: Arc::new(Mutex::new(rx_ctrl)),
        }
    }

    pub async fn start(&mut self) -> io::Result<()> {
        let rx_upstream = self.rx_upstream.clone();
        let socket_map = self.socket_map.clone();
        let flag_int = self.flag_interrupt.clone();
        let rx_control = self.rx_control.clone();
        tokio::spawn(async move {
            loop {
                {
                    if *flag_int.lock().await == true {
                        break;
                    }
                }
                {
                    if let Ok((job_id, sender)) = rx_upstream.lock().await.try_recv() {
                        match sender {
                            None => {
                                debug!(target: "atc-listener", "Removing {} from socket map", job_id);
                                socket_map.lock().await.remove(&job_id);
                            }
                            Some(sender) => {
                                debug!(target: "atc-listener", "Adding {} to socket map", job_id);
                                socket_map.lock().await.insert(job_id.clone(), sender);
                            }
                        };
                    }
                }

                {
                    if let Ok(server_cmd) = rx_control.lock().await.try_recv() {
                        if let ServerCommand::Terminate = server_cmd.clone() {
                            {
                                *flag_int.lock().await = true;
                            }
                            return;
                        }

                        let (target_job_id, msg) = match server_cmd.clone() {
                            ServerCommand::Message(t, m) => (t, m),
                            ServerCommand::Terminate => {
                                panic!("Need to be handled before entering this LOC")
                            }
                        };
                        for (job_id, sender) in socket_map.lock().await.iter() {
                            {
                                if *flag_int.lock().await == true {
                                    break;
                                }
                            }

                            if !target_job_id.is_none() && target_job_id.clone().unwrap() != *job_id
                            {
                                continue;
                            }

                            let job_id = job_id.clone();
                            let sender = sender.clone();

                            if let Err(e) = sender
                                .send(ChannelCommand::ChannelMessage((
                                    job_id.clone(),
                                    msg.clone(),
                                )))
                                .await
                            {
                                warn!(target: "atc-listener", "Error sending to message channel: {:?}", e);
                            } else {
                                debug!(target: "atc-listener", "Message sent to `{}`", job_id);
                            };
                        }
                    }
                }
            }
        });

        let uri = self.uri.clone();
        let server_started = self.server_started.clone();
        let tx_upstream = self.tx_upstream.clone();

        info!(target: "atc-listener", "Ready to start server `{}`:", uri.clone());
        *server_started.lock().await = true;
        if let Err(e) = create_listener(uri.clone(), tx_upstream, self.flag_interrupt.clone()).await
        {
            error!(target: "atc-listener", "Unable to bind to `{}`: `{:?}`", uri.clone(), e );
            *server_started.lock().await = false;
        }

        Ok(())
    }
}

pub struct Client {
    rx_control: Arc<Mutex<Receiver<ChannelCommand>>>,
    tx_control: Sender<ChannelCommand>,
    rx_message: Arc<Mutex<Receiver<ChannelCommand>>>,
    tx_message: Sender<ChannelCommand>,
    rx_outer_control: Arc<Mutex<Receiver<ServerCommand>>>,
    uri: String,
    pub id: String,
    callback_handler: Arc<Mutex<Option<ClientCallbackHandler>>>,
    flag_interupt: Arc<Mutex<bool>>,
}

pub enum ClientCallbackHandler {
    Closure(Box<dyn FnMut(String, String) + Send>),
    Channel(Sender<(String, String)>),
}

impl Client {
    pub fn new(uri: String, id: String, rx_outer_ctrl: Receiver<ServerCommand>) -> Self {
        let (tx_ctrl, rx_ctrl) = channel::<ChannelCommand>(1);
        let (tx_msg, rx_msg) = channel::<ChannelCommand>(1);
        Self {
            rx_control: Arc::new(Mutex::new(rx_ctrl)),
            tx_control: tx_ctrl,
            rx_message: Arc::new(Mutex::new(rx_msg)),
            tx_message: tx_msg,
            uri,
            id: id,
            callback_handler: Arc::new(Mutex::new(None)),
            rx_outer_control: Arc::new(Mutex::new(rx_outer_ctrl)),
            flag_interupt: Arc::new(Mutex::new(false)),
        }
    }

    pub async fn callback(self, cb: impl FnMut(String, String) + Send + 'static) -> Self {
        *self.callback_handler.lock().await = Some(ClientCallbackHandler::Closure(Box::new(cb)));
        self
    }

    pub async fn sender(self, sender: Sender<(String, String)>) -> Self {
        *self.callback_handler.lock().await = Some(ClientCallbackHandler::Channel(sender));
        self
    }

    pub async fn connect(&mut self) -> io::Result<()> {
        // Create another async task that always execute.
        // Move the message channel receiver and move a clone of control channel
        // sender into the task
        let tx_ctrl = self.tx_control.clone();
        let rx_msg = self.rx_message.clone();
        let rx_outer_ctrl = self.rx_outer_control.clone();
        let callback_handler = self.callback_handler.clone();
        let flag_int = self.flag_interupt.clone();
        tokio::spawn(async move {
            let mut last_ping = Instant::now();

            loop {
                {
                    if *flag_int.lock().await == true {
                        return;
                    }
                }
                if last_ping.elapsed().as_secs() >= 5 {
                    match tx_ctrl.send(ChannelCommand::Ping).await {
                        Ok(_) => {
                            last_ping = Instant::now();
                        }
                        Err(e) => {
                            error!(target: "atc-connector", "Unable to initualize PING command: {:?}", e);
                            return;
                        }
                    };
                }
                {
                    if let Ok(data) = rx_outer_ctrl.lock().await.try_recv() {
                        if data == ServerCommand::Terminate {
                            *flag_int.lock().await = true;
                            return;
                        }
                    }
                }

                {
                    if let Ok(data) = rx_msg.lock().await.try_recv() {
                        if let ChannelCommand::ChannelMessage((job_id, message)) = data.clone() {
                            if let Some(handler) = callback_handler.lock().await.as_mut() {
                                match handler {
                                    ClientCallbackHandler::Closure(closure) => {
                                        closure(job_id, message)
                                    }
                                    ClientCallbackHandler::Channel(sender) => {
                                        sender.send((job_id, message)).await.unwrap()
                                    }
                                }
                            }
                        }
                    };
                }
            }
        });

        let id = self.id.clone();
        let rx_ctrl = self.rx_control.clone();
        let tx_msg = self.tx_message.clone();
        let uri = self.uri.clone();
        let flag_int_clone = self.flag_interupt.clone();

        tokio::spawn(async move {
            if let Err(e) = create_connector(uri.clone(), id, rx_ctrl, tx_msg, flag_int_clone).await {
                error!(target: "atc-connector", "Unable to connect to remote server `{}`: {:?}",uri, e);
            }
        }).await.unwrap();

        Ok(())
    }
}
