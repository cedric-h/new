#![feature(drain_filter)]
use comn::Chat;
use std::time::Duration;

mod net;
use net::{open_socket, Session};

fn main() {
    pretty_env_logger::init();
    smol::block_on(start());
}

pub struct ChatDispatcher {
    /// All the chats accumulated in a single frame of client handling
    frame: Vec<Chat>,
}
impl ChatDispatcher {
    fn new() -> Self {
        Self { frame: Vec::with_capacity(10) }
    }

    fn fill<'a>(&mut self, clients: impl Iterator<Item = &'a mut Session>) {
        self.frame.clear();
        for Session { channel, addr, .. } in clients {
            while let Some(Chat(chat)) = channel.recv() {
                log::info!("{} said {}", addr, chat);
                self.frame.push(Chat(chat));
            }
        }
    }

    fn sync(&self, Session { channel, .. }: &mut Session) {
        for chat in &self.frame {
            comn::send_or_err(channel, chat.clone());
        }
    }
}

use glam::Vec2;
use hecs::Bundle;
#[derive(Debug, Bundle)]
struct PlayerIsland {
    pos: Vec2,
    art: comn::Art,
    session: Session,
}
impl PlayerIsland {
    fn new(pos: Vec2, session: Session) -> Self {
        Self { pos, session, art: comn::Art::Island }
    }
}

struct LastPos(Vec2);
struct LastPosTracker {
    need_last: Vec<(hecs::Entity, Vec2)>,
    messages: Vec<comn::Move>,
}
impl LastPosTracker {
    fn new() -> Self {
        Self { need_last: Vec::with_capacity(1000), messages: Vec::with_capacity(1000) }
    }

    fn track(&mut self, ecs: &mut Ecs, tick: u32) {
        let Self { messages, need_last } = self;
        need_last.extend(ecs.query::<&_>().without::<LastPos>().iter().map(|(e, p)| (e, *p)));
        for (e, pos) in need_last.drain(..) {
            comn::or_err!(ecs.insert_one(e, LastPos(pos)));
        }

        messages.clear();
        for (e, (&pos, last_pos)) in &mut ecs.query::<(&Vec2, &mut LastPos)>() {
            if pos != last_pos.0 {
                last_pos.0 = pos;
                messages.push(comn::Move { id: e.to_bits(), tick, pos });
            }
        }
    }

    fn sync(&self, Session { channel, .. }: &mut Session) {
        for &message in &self.messages {
            comn::send_or_err(channel, message);
        }
    }
}

struct Ecs(hecs::World);
impl Ecs {
    fn new() -> Self {
        Self(hecs::World::new())
    }

    fn clients(&self) -> hecs::QueryBorrow<'_, &Session> {
        self.0.query()
    }

    fn clients_mut(&mut self) -> hecs::QueryBorrow<'_, &mut Session> {
        self.0.query()
    }

    fn client_count(&self) -> usize {
        self.clients().iter().count()
    }

    /// Inserts the given island, returning its Entity.
    /// Sends a message to all clients letting them know as much.
    fn add_island(&mut self, island: PlayerIsland) -> hecs::Entity {
        use comn::{send_or_err, EntEvent};
        let ent = self.0.reserve_entity();
        let spawn_msg = EntEvent::Spawn(ent.to_bits(), island.pos, island.art);

        comn::or_err!(self.0.insert(ent, island));
        for (_, Session { channel, .. }) in self.clients_mut().iter().filter(|(e, _)| *e != ent) {
            send_or_err(channel, spawn_msg)
        }
        ent
    }

    /// Removes an island by its Id, sending a message to all clients encouraging
    /// them to delete it.
    ///
    /// Returns the island.
    fn remove_island(&mut self, ent: hecs::Entity) -> Result<PlayerIsland, hecs::ComponentError> {
        for (_, Session { channel, .. }) in &mut self.clients_mut() {
            comn::send_or_err(channel, comn::EntEvent::Despawn(ent.to_bits()));
        }
        let island = self.0.remove(ent);
        if let Err(e) = self.0.despawn(ent) {
            log::error!("couldn't remove ent: {}", e);
        }
        island
    }
}
impl std::ops::Deref for Ecs {
    type Target = hecs::World;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Ecs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

struct World {
    name: String,
    ecs: Ecs,
    last_pos_tracker: LastPosTracker,
    tick: u32,

    /// Temporary buffer for storing clients before removing them.
    timed_out: Vec<hecs::Entity>,
}
impl World {
    fn new(name: impl ToString) -> Self {
        Self {
            name: name.to_string(),
            ecs: Ecs::new(),
            last_pos_tracker: LastPosTracker::new(),
            tick: 0,
            timed_out: Vec::with_capacity(10),
        }
    }

    /// Add a client and their island to this world,
    /// sending them an intitial WorldJoin packet with essential world state.
    fn connect(&mut self, island: PlayerIsland) {
        use comn::{send_or_err, WorldJoin};
        let Self { name, ecs, tick, .. } = self;

        log::info!(
            "{} > {} joined in! world clients: {}",
            name,
            island.session.addr,
            ecs.client_count() + 1
        );

        let ent = ecs.add_island(island);
        let islands =
            ecs.query::<(&_, &_)>().iter().map(|(e, (&p, &a))| (e.to_bits(), p, a)).collect();

        send_or_err(
            &mut ecs.get_mut::<Session>(ent).unwrap().channel,
            WorldJoin {
                world_name: name.clone(),
                islands,
                your_island: ent.to_bits(),
                tick: *tick,
            },
        );
    }

    fn update(&mut self, chat: &mut ChatDispatcher) {
        let Self { last_pos_tracker, ecs, timed_out, name, tick, .. } = self;
        *tick += 1;

        last_pos_tracker.track(ecs, *tick);
        for (e, client) in &mut ecs.clients_mut() {
            if client.heartbeat() {
                timed_out.push(e);
            }
            last_pos_tracker.sync(client);
            chat.sync(client);
            client.channel.flush_all();
        }

        for timed_out in timed_out.drain(..) {
            let island = ecs.remove_island(timed_out).unwrap();
            log::info!(
                "{} > {} timed out! world clients: {}",
                name,
                island.session.addr,
                ecs.client_count()
            );
        }
    }

    /// Returns `true` if any clients are connected
    fn is_occupied(&self) -> bool {
        self.ecs.client_count() > 0
    }

    /// Removes all islands, etc. without notifying any collected clients.
    /// Use with caution.
    fn clear(&mut self) {
        self.ecs.clear();
    }
}

use std::time::Instant;
#[derive(Debug)]
struct Revolve {
    center: Vec2,
    start: Instant,
}
impl Revolve {
    #[allow(dead_code)]
    fn new(center: Vec2) -> Self {
        Self { center, start: Instant::now() }
    }
    fn offset(center: Vec2, offset: f32) -> Self {
        Self { center, start: Instant::now() - Duration::from_secs_f32(offset) }
    }
}

fn revolve(ecs: &mut hecs::World) {
    for (_, (pos, &Revolve { center, start })) in &mut ecs.query::<(&mut Vec2, &_)>() {
        let dist = (center - *pos).length();
        *pos = center + dist * comn::angle_to_vec(start.elapsed().as_secs_f32());
    }
}

fn prepare_starter(world: &mut World) {
    world.clear();
    const MAX: usize = 1;
    for i in 0..MAX {
        use std::f32::consts::TAU;
        world.ecs.spawn((
            Vec2::one(),
            comn::Art::Vase,
            Revolve::offset(Vec2::zero(), i as f32 / MAX as f32 * TAU),
        ));
    }
}

struct StarterWorlds {
    worlds: Vec<World>,
}
impl StarterWorlds {
    fn new() -> Self {
        Self { worlds: Vec::with_capacity(10) }
    }

    /// Connects a client to a Starter World, reusing an old one if
    /// an empty one is available and allocating a new one otherwise.
    fn connect(&mut self, client: Session) {
        let island = PlayerIsland::new(Vec2::zero(), client);
        if let Some(world) = self.unoccupied_mut().next() {
            prepare_starter(world);
            world.connect(island);
            return; // return here placates borrowck
        }

        let worlds = &mut self.worlds;
        let mut new_world = World::new(format!("Starter World {}", worlds.len()));
        prepare_starter(&mut new_world);
        new_world.connect(island);
        worlds.push(new_world);
    }

    fn update(&mut self, chat: &mut ChatDispatcher) {
        for world in &mut self.worlds {
            world.update(chat);
            revolve(&mut world.ecs);
        }
    }

    /// Returns an iterator over mutable references to the worlds
    /// that have clients in them.
    pub fn occupied_mut(&mut self) -> impl Iterator<Item = &mut World> {
        self.worlds.iter_mut().filter(|world| world.is_occupied())
    }

    /// Returns an iterator over mutable references to the worlds
    /// that DO NOT have clients in them.
    pub fn unoccupied_mut(&mut self) -> impl Iterator<Item = &mut World> {
        self.worlds.iter_mut().filter(|world| !world.is_occupied())
    }
}

/// Technically a Quadratic Bezier Curve.
/// that won't stop me from calling it a "three_lerp" or a thlerp for short :)
fn thlerp(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> Vec2 {
    p0.lerp(p1, t).lerp(p1.lerp(p2, t), t)
}

async fn start() {
    let mut chat = ChatDispatcher::new();
    let mut starter_worlds = StarterWorlds::new();
    let (client_tx, client_rx) = std::sync::mpsc::sync_channel(100);

    smol::spawn(open_socket(comn::SERVER, 2500, client_tx)).detach();

    let mut step_time = Instant::now();
    loop {
        // Add any new clients to our collection of channels
        if let Ok(session) = client_rx.try_recv() {
            starter_worlds.connect(session);
        }

        for world in starter_worlds.occupied_mut() {
            chat.fill(world.ecs.clients_mut().iter().map(|(_, s)| s));
        }
        starter_worlds.update(&mut chat);

        step_time += Duration::from_millis(50);
        smol::Timer::at(step_time).await;
    }
}
