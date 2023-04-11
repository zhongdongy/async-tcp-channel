pub mod libs;

pub use libs::{
    command::{ChannelCommand, ServerCommand},
    connector::create_connector,
    frame::Frame,
    listener::create_listener,
};

use log::{debug, error, info, warn};
use queues::{IsQueue, Queue};
use std::{collections::BTreeMap, sync::Arc};
use tokio::{
    io,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
    time::Instant,
};

pub struct ChannelInfo {
    last_write: Instant,
    messages: Queue<String>,
}

impl ChannelInfo {
    pub fn new(message: String) -> Self {
        let mut messages: Queue<String> = Queue::new();
        messages.add(message).unwrap();
        Self {
            last_write: Instant::now(),
            messages,
        }
    }

    /// Add element into a queue.
    pub fn enqueue(&mut self, message: String) {
        self.messages.add(message).unwrap();
    }

    /// Get head element of a queue, this doesn't remove it from queue.
    pub fn head(&mut self) -> Option<String> {
        if let Ok(val) = self.messages.peek() {
            Some(val)
        } else {
            None
        }
    }

    pub fn update_instant(&mut self) {
        self.last_write = Instant::now();
    }

    /// You should call head to get the head element and then call this method
    /// to remove it from queue.
    pub fn dequeue(&mut self) -> bool {
        self.messages.remove().is_ok()
    }

    pub fn len(&self) -> usize {
        self.messages.size()
    }
}

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

        let message_queue: Arc<Mutex<BTreeMap<String, ChannelInfo>>> =
            Arc::new(Mutex::new(BTreeMap::new()));

        tokio::spawn(async move {
            loop {
                let flag_int_guard = flag_int.lock().await;
                if *flag_int_guard == true {
                    break;
                }
                drop(flag_int_guard);

                let mut rx_upstream_guard = rx_upstream.lock().await;
                if let Ok((channel_id, sender)) = rx_upstream_guard.try_recv() {
                    match sender {
                        None => {
                            debug!(target: "atc-listener", "Removing {} from socket map", channel_id);
                            socket_map.lock().await.remove(&channel_id);
                        }
                        Some(sender) => {
                            debug!(target: "atc-listener", "Adding {} to socket map", channel_id);
                            socket_map.lock().await.insert(channel_id.clone(), sender);
                        }
                    };
                }
                drop(rx_upstream_guard);

                // Handle queued messages.
                // For each channel with queued messages, only one message will
                // be sent to the channel.
                let mut message_queue_guard = message_queue.lock().await;
                let socket_map_guard = socket_map.lock().await;
                let mut should_drop_channel_ids = vec![];
                for (channel_id, channel_info) in message_queue_guard.iter_mut() {
                    if channel_info.last_write.elapsed().as_secs() > 60 {
                        // Last write operation was 60 seconds ago, should not
                        // queue this anymore, drop all existing messages.
                        should_drop_channel_ids.push(channel_id.clone());
                        warn!(target: "atc-listener", "Message queue of channel (`{}`) will be dropped due to inactivity for more than 60 seconds", channel_id);
                        continue;
                    }
                    if channel_info.len() > 128 {
                        // Too many queued messages, drop all existing messages.
                        should_drop_channel_ids.push(channel_id.clone());
                        warn!(target: "atc-listener", "Message queue of channel (`{}`) will be dropped due to exceeds 128 message limit", channel_id);
                        continue;
                    }
                    if socket_map_guard.contains_key(channel_id) && channel_info.len() > 0 {
                        // Should try send message.
                        let oldest_msg = channel_info.head().unwrap();
                        let sender = socket_map_guard.get(channel_id).unwrap().clone();
                        if let Ok(_) = sender
                            .send(ChannelCommand::ChannelMessage((
                                channel_id.clone(),
                                oldest_msg,
                            )))
                            .await
                        {
                            channel_info.dequeue();
                            channel_info.update_instant();
                            debug!(target: "atc-listener", "One queued message sent to existing channel `{}`.", channel_id);
                        } else {
                            warn!(target: "atc-listener", "One queued message not sent to existing channel `{}`.", channel_id);
                        }
                    }
                }
                should_drop_channel_ids.iter().for_each(|id| {
                    message_queue_guard.remove(id);
                });
                drop(socket_map_guard);
                drop(message_queue_guard);

                let mut rx_control_guard = rx_control.lock().await;
                if let Ok(server_cmd) = rx_control_guard.try_recv() {
                    if let ServerCommand::Terminate = server_cmd.clone() {
                        let mut flag_int_guard = flag_int.lock().await;
                        *flag_int_guard = true;
                        drop(flag_int_guard);
                        return;
                    }

                    let (target_channel_id, msg) = match server_cmd.clone() {
                        ServerCommand::Message(t, m) => (t, m),
                        ServerCommand::Terminate => {
                            panic!("Need to be handled before entering this LOC")
                        }
                    };

                    // Check if target channel id exists in `socket_map`, or
                    // Add to queue and wait for a while.
                    if let Some(target_channel_id) = target_channel_id {
                        let socket_map_guard = socket_map.lock().await;
                        if socket_map_guard.contains_key(&target_channel_id) {
                            let sender = socket_map_guard.get(&target_channel_id).unwrap().clone();
                            if let Err(e) = sender
                                .send(ChannelCommand::ChannelMessage((
                                    target_channel_id.clone(),
                                    msg.clone(),
                                )))
                                .await
                            {
                                warn!(target: "atc-listener", "Error sending to message channel [will be queued]: {:?}", e);

                                // Put to message queue.
                                let mut queue = message_queue.lock().await;
                                if queue.contains_key(&target_channel_id) {
                                    queue.get_mut(&target_channel_id).unwrap().enqueue(msg);
                                } else {
                                    queue.insert(target_channel_id, ChannelInfo::new(msg));
                                }
                                drop(queue);
                            } else {
                                debug!(target: "atc-listener", "Message sent to `{}`", target_channel_id);
                            };
                        } else {
                            // Channel ID doesn't exist, put into queue.
                            let mut queue = message_queue.lock().await;
                            if queue.contains_key(&target_channel_id) {
                                queue.get_mut(&target_channel_id).unwrap().enqueue(msg);
                            } else {
                                queue.insert(target_channel_id, ChannelInfo::new(msg));
                            }
                            drop(queue);
                        }
                        drop(socket_map_guard);
                    } else {
                        // Note: messages without target channel id will not be
                        // queued.
                    }
                }
                drop(rx_control_guard);
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
    should_reconnect: bool,
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
            should_reconnect: false,
        }
    }

    pub fn reconnect(self, should_reconnect: bool) -> Self {
        Self {
            should_reconnect,
            ..self
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
                        if let ChannelCommand::ChannelMessage((channel_id, message)) = data.clone()
                        {
                            if let Some(handler) = callback_handler.lock().await.as_mut() {
                                match handler {
                                    ClientCallbackHandler::Closure(closure) => {
                                        closure(channel_id, message)
                                    }
                                    ClientCallbackHandler::Channel(sender) => {
                                        sender.send((channel_id, message)).await.unwrap()
                                    }
                                }
                            }
                        }
                    };
                }
            }
        });
        let mut reconnect_attempts = 0;
        loop {
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

            if !self.should_reconnect || *self.flag_interupt.lock().await {
                warn!(target: "atc-connector", "Reconnect not enabled or user requested termination from client side.");
                break;
            }
            if reconnect_attempts> 8 {
                warn!(target: "atc-connector", "No more reconnecting after 8 attempts.");
                break;
            }

            // `should_reconnect` flag set to true, and interrupt flag not set
            // will restart another connection.
            info!(target: "atc-connector", "Client connection restarting");
            reconnect_attempts+=1;
        }
        Ok(())
    }
}
