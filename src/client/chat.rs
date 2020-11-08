use comn::Chat;
use macroquad::prelude::*;
use turbulence::MessageChannels;

const MAX_MESSAGES: usize = 100;
pub struct ChatBox {
    /// All of the messages from the server
    history: [String; MAX_MESSAGES],
    /// Where new messages from the server should be written to
    pen: usize,
    /// The message the user is preparing to send
    wip_message: String,
    send_wip: bool,
    jump_to_bottom: bool,
}
impl ChatBox {
    pub fn new() -> Self {
        Self {
            history: [(); MAX_MESSAGES].map(|_| Default::default()),
            pen: 1,
            wip_message: String::with_capacity(500),
            send_wip: false,
            jump_to_bottom: false,
        }
    }

    pub fn log_message(&mut self, message: String) {
        let Self { pen, history, .. } = self;
        history[*pen] = message;
        *pen += 1;
        *pen = if *pen == MAX_MESSAGES { 0 } else { *pen };
    }

    pub fn sync_messages(&mut self, channels: &mut MessageChannels) {
        while let Some(Chat(chat)) = channels.recv() {
            self.log_message(chat);
        }

        let Self { send_wip, wip_message, .. } = self;
        if *send_wip {
            *send_wip = false;
            comn::send_or_err(channels, Chat(std::mem::take(wip_message)));
        }
    }

    pub fn ui(&mut self) {
        use megaui::{hash, widgets::Group, Layout, Vector2};
        use megaui_macroquad::{draw_window, megaui, WindowParams};
        let Self { send_wip, jump_to_bottom, wip_message, pen, history, .. } = self;

        const CHAT_WIDTH: f32 = 400.0;
        draw_window(
            hash!(),
            Vec2::zero(),
            vec2(CHAT_WIDTH, 200.0),
            WindowParams { label: "chat".to_string(), ..Default::default() },
            |ui| {
                let mut jump_to_bottom_button = false;
                Group::new(hash!(), Vector2::new(CHAT_WIDTH, 160.0))
                    .layout(Layout::Free(Vector2::new(0.0, 0.0)))
                    .ui(ui, |ui| {
                        for msg_i in (*pen..MAX_MESSAGES).chain(0..*pen) {
                            let msg = &*history[msg_i].trim();
                            ui.label(None, msg);
                        }
                        if ui.frame == 1 || *jump_to_bottom {
                            ui.scroll_here();
                            *jump_to_bottom = false;
                        }

                        jump_to_bottom_button = -ui.scroll().y < ui.scroll_max().y;
                    });

                if jump_to_bottom_button {
                    *jump_to_bottom =
                        ui.button(Vector2::new(CHAT_WIDTH - 118.5, 5.0), "Jump to Bottom");
                }

                Group::new(hash!(), Vector2::new(CHAT_WIDTH, 25.0))
                    .layout(Layout::Free(Vector2::new(0.0, 160.0)))
                    .ui(ui, |ui| ui.input_text(hash!(), "<- msg", wip_message));

                if is_key_pressed(KeyCode::Enter) && !wip_message.is_empty() {
                    *send_wip = true;
                }
            },
        );
    }
}
