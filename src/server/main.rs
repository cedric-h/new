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
use std::net::SocketAddr;
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

struct World {
    name: String,
    ecs: hecs::World,

    /// Temporary buffer for storing clients before removing them.
    timed_out: Vec<hecs::Entity>,
}
impl World {
    fn new(name: impl ToString) -> Self {
        Self { name: name.to_string(), ecs: hecs::World::new(), timed_out: Vec::with_capacity(10) }
    }

    fn clients(&self) -> hecs::QueryBorrow<'_, &Session> {
        self.ecs.query()
    }

    fn clients_mut(&mut self) -> hecs::QueryBorrow<'_, &mut Session> {
        self.ecs.query()
    }

    fn client_count(&self) -> usize {
        self.clients().iter().count()
    }

    /// Add a client and their island to this world,
    /// sending them an intitial WorldJoin packet with essential world state.
    fn connect(&mut self, island: PlayerIsland) {
        use comn::{send_or_err, WorldJoin};
        log::info!(
            "{} > {} joined in! world clients: {}",
            self.name,
            island.session.addr,
            self.client_count() + 1
        );

        let ent = self.add_island(island);
        let islands =
            self.ecs.query::<(&_, &_)>().iter().map(|(e, (&p, &a))| (e.to_bits(), p, a)).collect();

        send_or_err(
            &mut self.ecs.get_mut::<Session>(ent).unwrap().channel,
            WorldJoin { world_name: self.name.clone(), islands, your_island: ent.to_bits() },
        );
    }

    fn update(&mut self, chat: &mut ChatDispatcher) {
        let mut timed_out = std::mem::take(&mut self.timed_out);
        for (e, client) in &mut self.clients_mut() {
            if client.heartbeat() {
                timed_out.push(e);
            }
            chat.sync(client);
            client.channel.flush_all();
        }

        for timed_out in timed_out.drain(..) {
            let island = self.remove_island(timed_out).unwrap();
            log::info!(
                "{} > {} timed out! world clients: {}",
                self.name,
                island.session.addr,
                self.client_count()
            );
        }

        self.timed_out = timed_out;
    }

    /// Inserts the given island, returning its Entity.
    /// Sends a message to all clients letting them know as much.
    fn add_island(&mut self, island: PlayerIsland) -> hecs::Entity {
        use comn::{send_or_err, EntEvent};
        let ent = self.ecs.reserve_entity();
        let spawn_msg = EntEvent::Spawn(ent.to_bits(), island.pos, island.art);

        comn::or_err!(self.ecs.insert(ent, island));
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
        let island = self.ecs.remove(ent);
        if let Err(e) = self.ecs.despawn(ent) {
            log::error!("couldn't remove ent: {}", e);
        }
        island
    }

    /// Returns `true` if any clients are connected
    fn is_occupied(&self) -> bool {
        self.client_count() > 0
    }

    /// Removes all islands, etc. without notifying any collected clients.
    /// Use with caution.
    fn clear(&mut self) {
        self.ecs.clear();
    }
}

fn prepare_starter(world: &mut World) {
    world.clear();
    const MAX: usize = 3;
    for i in 0..MAX {
        use std::f32::consts::TAU;
        world.ecs.spawn((comn::angle_to_vec(i as f32 / MAX as f32 * TAU), comn::Art::Vase));
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

async fn start() {
    let mut chat = ChatDispatcher::new();
    let mut starter_worlds = StarterWorlds::new();
    let (client_tx, client_rx) = std::sync::mpsc::sync_channel(100);

    smol::spawn(open_socket(comn::SERVER, 2500, client_tx)).detach();

    loop {
        // Add any new clients to our collection of channels
        if let Ok(session) = client_rx.try_recv() {
            starter_worlds.connect(session);
        }

        for world in starter_worlds.occupied_mut() {
            chat.fill(world.clients_mut().iter().map(|(_, s)| s));
        }
        starter_worlds.update(&mut chat);

        smol::Timer::after(Duration::from_millis(16)).await;
    }
}
