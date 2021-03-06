use super::{Chat, Heartbeat};
use smol::stream::StreamExt;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use turbulence::{
    message_channels::ChannelMessage, reliable_channel, BufferPacket, BufferPacketPool, BufferPool,
    MessageChannelMode, MessageChannelSettings, MessageChannels, MessageChannelsBuilder,
    PacketMultiplexer, Runtime,
};

/// Port 0 here should get the OS to give us an open port
pub const CLIENT: &str = "127.0.0.1:0";
pub const SERVER: &str = "127.0.0.1:1337";

pub fn send_or_err<M: ChannelMessage + std::fmt::Debug>(channels: &mut MessageChannels, m: M) {
    if let Some(rejected) = channels.send(m) {
        log::error!("channel rejected message: {:#?}", rejected);
    }
}

macro_rules! messages {
    ( use { $($use:tt)* }; $( (
        MessageChannelSettings {
            $($settings:tt)*
        }
        #[$derive:meta]
        pub $what:ident $name:ident $($rest:tt)*
    ), )* ) => {
        pub(super) mod messages {
            use { $( $use )* };
            $(
                #[$derive]
                pub $what $name $( $rest )*
            )*
        }
        use messages::*;

        #[repr(u8)]
        enum Channel {
            $( $name, )*
        }

        /// Creates a MessageChannels configured with our message types, and a multiplexer
        /// for sending messages into the channels
        pub fn channel_with_multiplexer(
            pool: BufferPacketPool<SimpleBufferPool>,
        ) -> (MessageChannels, PacketMultiplexer<BufferPacket<Box<[u8]>>>) {
            let mut multiplexer = PacketMultiplexer::new();
            let mut builder = MessageChannelsBuilder::new(GlobalSmolRuntime, pool);

            $(
            builder
                .register::<$name>(MessageChannelSettings {
                    channel: Channel::$name as u8,
                    $( $settings )*
                })
                .expect(concat!("couldn't register ", stringify!($name)));
            )*

            (builder.build(&mut multiplexer), multiplexer)
        }
    }
}

const SENSIBLE_RELIABLE: MessageChannelMode = MessageChannelMode::Reliable {
    reliability_settings: reliable_channel::Settings {
        bandwidth: 4096,
        recv_window_size: 1024,
        send_window_size: 1024,
        burst_bandwidth: 1024,
        init_send: 512,
        wakeup_time: Duration::from_millis(100),
        initial_rtt: Duration::from_millis(200),
        max_rtt: Duration::from_secs(2),
        rtt_update_factor: 0.1,
        rtt_resend_factor: 1.5,
    },
    max_message_len: 1024,
};

messages! {
    use {
        serde::{Serialize, Deserialize},
        glam::Vec2,
    };
    (
        MessageChannelSettings {
            channel_mode: MessageChannelMode::Unreliable,
            message_buffer_size: 8,
            packet_buffer_size: 8,
        }
        #[derive(Serialize, Deserialize, Debug)]
        pub struct Heartbeat;
    ),
    (
        MessageChannelSettings {
            channel_mode: SENSIBLE_RELIABLE,
            message_buffer_size: 8,
            packet_buffer_size: 8,
        }
        #[derive(Serialize, Deserialize, Clone, Debug)]
        pub struct Chat(pub String);
    ),
    (
        MessageChannelSettings {
            channel_mode: MessageChannelMode::Unreliable,
            message_buffer_size: 8,
            packet_buffer_size: 8,
        }
        #[derive(Serialize, Deserialize, Debug, Clone, Copy)]
        pub struct Move {
            pub id: u64,
            pub tick: u32,
            pub pos: Vec2
        }
    ),
    (
        MessageChannelSettings {
            channel_mode: SENSIBLE_RELIABLE,
            message_buffer_size: 8,
            packet_buffer_size: 8,
        }
        #[derive(Serialize, Deserialize, Debug, Clone, Copy)]
        pub enum EntEvent {
            Spawn(u64, Vec2, crate::Art),
            Despawn(u64),
        }
    ),
    (
        MessageChannelSettings {
            channel_mode: SENSIBLE_RELIABLE,
            message_buffer_size: 8,
            packet_buffer_size: 8,
        }
        #[derive(Serialize, Deserialize, Debug)]
        pub struct WorldJoin {
            pub islands: Vec<(u64, Vec2, crate::Art)>,
            pub your_island: u64,
            pub world_name: String,
            pub tick: u32,
        }
    ),
}

/// Spawns a new task which sends all packages from an Outgoing channel into a UDP socket.
pub fn send_outgoing_to_socket(
    mut outgoing: turbulence::OutgoingMultiplexedPackets<BufferPacket<Box<[u8]>>>,
    socket: smol::net::UdpSocket,
    to: std::net::SocketAddr,
) {
    GlobalSmolRuntime.spawn(async move {
        while let Some(p) = outgoing.next().await {
            if let Err(e) = socket.send_to(&p, to).await {
                println!("couldn't send: {}", e);
            }
        }
    });
}

/// A smol::Timer wrapped to produce `()` instead of `Instant`,
/// for compatibility with the turbulence::Runtime trait.
pub struct Timer(pub smol::Timer);
impl Future for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use smol::future::FutureExt;
        self.0.poll(cx).map(|_| ())
    }
}

#[derive(Clone, Debug, Default)]
/// Facilitates using Smol's global executor with turbulence
pub struct GlobalSmolRuntime;
impl turbulence::Runtime for GlobalSmolRuntime {
    type Instant = Instant;
    type Sleep = Timer;

    fn spawn<F: Future<Output = ()> + Send + 'static>(&self, fut: F) {
        smol::spawn(fut).detach()
    }

    fn now(&self) -> Self::Instant {
        Instant::now()
    }

    fn elapsed(&self, instant: Self::Instant) -> Duration {
        instant.elapsed()
    }

    fn duration_between(&self, earlier: Self::Instant, later: Self::Instant) -> Duration {
        later.duration_since(earlier)
    }

    fn sleep(&self, d: Duration) -> Self::Sleep {
        Timer(smol::Timer::after(d))
    }
}

#[derive(Clone, Debug)]
pub struct SimpleBufferPool(pub usize);

impl BufferPool for SimpleBufferPool {
    type Buffer = Box<[u8]>;

    fn acquire(&self) -> Self::Buffer {
        vec![0; self.0].into_boxed_slice()
    }
}

/// Returns a new Packet preallocated with an entire packet's worth of memory.
pub fn acquire_max(
    pool: &turbulence::BufferPacketPool<SimpleBufferPool>,
) -> turbulence::BufferPacket<Box<[u8]>> {
    use turbulence::{Packet, PacketPool};

    let mut packet = pool.acquire();
    packet.resize(1024, 0);
    packet
}
