use comn::Heartbeat;
use std::{net::SocketAddr, sync::mpsc::SyncSender, time::Instant};
use turbulence::MessageChannels;

#[derive(Debug)]
pub struct Session {
    pub channel: MessageChannels,
    pub addr: SocketAddr,
    pub heartbeat: std::time::Instant,
}
impl Session {
    pub fn new(channel: MessageChannels, addr: SocketAddr) -> Self {
        Self { channel, addr, heartbeat: Instant::now() }
    }

    /// Returns true if the user has timed out
    pub fn heartbeat(&mut self) -> bool {
        let Self { channel, heartbeat, .. } = self;

        // Manage client heartbeats, boot out the timeouts.
        if let Some(Heartbeat) = channel.recv() {
            *heartbeat = Instant::now();
        }

        heartbeat.elapsed().as_secs_f32() > 3.0
    }
}

/// A UDP socket that accepts new connections for as long as it's open.
pub async fn open_socket(my_addr: &str, pool_size: usize, client_tx: SyncSender<Session>) {
    use comn::net::{
        acquire_max, channel_with_multiplexer, send_outgoing_to_socket, SimpleBufferPool,
    };
    use std::collections::HashMap;
    use turbulence::{BufferPacketPool, Packet};

    let pool = BufferPacketPool::new(SimpleBufferPool(pool_size));
    let mut sockets_incoming = HashMap::with_capacity(100);

    let socket = smol::net::UdpSocket::bind(my_addr).await.expect("couldn't bind to address");

    loop {
        let mut packet = acquire_max(&pool);
        match socket.recv_from(&mut packet).await {
            Ok((len, addr)) => {
                let incoming = sockets_incoming.entry(addr).or_insert_with(|| {
                    let (channel, multiplexer) = channel_with_multiplexer(pool.clone());
                    let (incoming, outgoing) = multiplexer.start();
                    send_outgoing_to_socket(outgoing, socket.clone(), addr);
                    client_tx.send(Session::new(channel, addr)).unwrap();
                    incoming
                });
                packet.truncate(len);
                use turbulence::packet_multiplexer::{IncomingError::*, IncomingTrySendError::*};
                match incoming.try_send(packet) {
                    Ok(()) => {}
                    Err(Error(ChannelReceiverDropped)) => return,
                    Err(e) => log::error!("couldn't send packet: {}", e),
                }
            }
            Err(e) => log::error!("couldn't recieve packet from UDP socket: {}", e),
        };
    }
}
