#![feature(array_map)]
use comn::Heartbeat;
use macroquad::prelude::*;
use std::time::Instant;
use turbulence::MessageChannels;

mod chat;
use chat::ChatBox;

#[derive(Debug, Copy, Clone)]
struct Sprite {
    rect: Rect,
    art: comn::Art,
}
impl Sprite {
    fn new(art: comn::Art) -> Self {
        Self {
            art,
            rect: match art {
                comn::Art::Island => match rand::rand() % 3 {
                    0 => Rect { x: 000.0, y: 000.0, w: 256.0, h: 256.0 },
                    1 => Rect { x: 000.0, y: 256.0, w: 256.0, h: 256.0 },
                    2 => Rect { x: 256.0, y: 000.0, w: 256.0, h: 256.0 },
                    _ => unreachable!(),
                },
                comn::Art::Vase => Rect { x: 256.0, y: 256.0, w: 256.0, h: 256.0 },
            },
        }
    }
}

#[derive(Debug, Copy, Clone)]
struct Ent {
    pos_frames: [(f64, Vec2); 2],
    last_update: Instant,
    sprite: Sprite,
}
impl Ent {
    fn new(pos: Vec2, art: comn::Art) -> Self {
        Self { pos_frames: [(0.0, pos); 2], sprite: Sprite::new(art), last_update: Instant::now() }
    }

    //dbg!(between_before_and_last, since_last, ratio);
    fn pos_lerp(&self) -> Vec2 {
        let [(last_time, last), (before_time, before)] = self.pos_frames;
        before.lerp(
            last,
            (self.last_update.elapsed().as_secs_f64() / (last_time - before_time)) as f32,
        )
    }
}

struct Ents {
    pub ents: fxhash::FxHashMap<u64, Ent>,
}
impl Ents {
    pub fn new(mut islands: Vec<(u64, Vec2, comn::Art)>) -> Self {
        use {fxhash::FxBuildHasher, std::collections::HashMap};
        let mut ents = HashMap::with_capacity_and_hasher(1000, FxBuildHasher::default());
        ents.extend(islands.drain(..).map(|(i, p, a)| (i, Ent::new(p, a))));
        Self { ents }
    }

    pub fn poll_messages(&mut self, channels: &mut MessageChannels) {
        use comn::{EntEvent, Move};
        let Self { ents, .. } = self;
        while let Some(e) = channels.recv() {
            match dbg!(e) {
                EntEvent::Spawn(id, pos, art) => ents.insert(id, Ent::new(pos, art)),
                EntEvent::Despawn(id) => ents.remove(&id),
            };
        }
        while let Some(Move { id, time, pos }) = channels.recv() {
            if let Some(ent) = ents.get_mut(&id) {
                let [(last_time, last), _] = ent.pos_frames;
                if time > last_time {
                    ent.pos_frames = [(time, pos), (last_time, last)];
                    ent.last_update = Instant::now();
                }
            }
        }
    }
}

struct Drawer {
    atlas: Texture2D,
}
impl Drawer {
    pub async fn new() -> Self {
        Self { atlas: load_texture("atlas.png").await }
    }

    pub fn draw(&self, arts: impl Iterator<Item = (Vec2, Sprite)>) {
        use macroquad::prelude::*;

        set_camera(Camera2D {
            zoom: vec2(1.0, screen_width() / screen_height()) / 4.8,
            ..Default::default()
        });

        clear_background(Color([180, 227, 245, 255]));

        for (pos, Sprite { rect: image_source, art }) in arts {
            let mut world_size = Vec2::one();
            world_size *= vec2(1.0, -1.0) * art.size();
            draw_texture_ex(
                self.atlas,
                pos.x() - world_size.x() / 2.0,
                pos.y() - world_size.y() / 2.0,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(world_size),
                    source: Some(image_source),
                    ..Default::default()
                },
            )
        }

        set_default_camera();
    }
}

fn loading_text(t: &'static str) {
    clear_background(BLACK);
    draw_text(t, 20.0, 20.0, 40.0, WHITE);
}

struct Heart(Instant);
impl Heart {
    fn new() -> Self {
        Self(Instant::now() - std::time::Duration::from_secs(1))
    }

    fn beat(&mut self, channel: &mut MessageChannels) {
        if self.0.elapsed().as_secs_f32() > 0.2 {
            self.0 = Instant::now();
            channel.send(Heartbeat);
        }
    }
}

fn window_config() -> Conf {
    Conf {
        window_width: 1080,
        window_height: 720,
        window_title: String::from("new!"),
        ..Default::default()
    }
}
#[macroquad::main(window_config)]
async fn main() {
    #[cfg(target_arch = "wasm32")]
    sapp_console_log::init().unwrap();
    #[cfg(not(target_arch = "wasm32"))]
    pretty_env_logger::init();

    let mut channel = direct_socket(comn::CLIENT, comn::SERVER, 1024);
    let mut heart = Heart::new();
    let intro: comn::WorldJoin = loop {
        if let Some(intro) = channel.recv() {
            break intro;
        }

        heart.beat(&mut channel);
        channel.flush::<Heartbeat>();

        loading_text("connecting to server ...");
        next_frame().await;
    };
    let mut ents = Ents::new(intro.islands);
    ents.ents.remove(&intro.your_island); // DELETE ME PLS
    let mut chat_box = ChatBox::new();
    chat_box.log_message(format!("Welcome to {}!", intro.world_name));

    loading_text("loading texture ..");
    let drawer = Drawer::new().await;

    loop {
        heart.beat(&mut channel);

        chat_box.sync_messages(&mut channel);
        ents.poll_messages(&mut channel);
        channel.flush_all();

        drawer.draw(ents.ents.values().map(|e| (e.pos_lerp(), e.sprite)));
        chat_box.ui();
        megaui_macroquad::draw_megaui();

        next_frame().await;
    }
}

// Returns a MessageChannels corresponding to a UDP socket that only accepts messages from,
// and sends messages to, a single address.
fn direct_socket(
    my_addr: &'static str,
    remote_addr: &'static str,
    pool_size: usize,
) -> MessageChannels {
    use comn::net::{
        acquire_max, channel_with_multiplexer, send_outgoing_to_socket, SimpleBufferPool,
    };
    use turbulence::{BufferPacketPool, Packet};

    let pool = BufferPacketPool::new(SimpleBufferPool(pool_size));
    let (channel, multiplexer) = channel_with_multiplexer(pool.clone());

    let socket = smol::block_on(async {
        let s = smol::net::UdpSocket::bind(my_addr).await.expect("couldn't bind to address");
        s.connect(remote_addr).await.expect("connect function failed");
        s
    });

    let (mut incoming, outgoing) = multiplexer.start();
    send_outgoing_to_socket(outgoing, socket.clone(), remote_addr.parse().unwrap());

    smol::spawn(async move {
        loop {
            let mut packet = acquire_max(&pool);
            match socket.recv(&mut packet).await {
                Ok(len) => {
                    packet.truncate(len);
                    if let Err(e) = incoming.try_send(packet) {
                        error!("couldn't send packet: {}", e);
                    }
                }
                Err(e) => error!("couldn't recieve packet from UDP socket: {}", e),
            };
        }
    })
    .detach();

    channel
}
