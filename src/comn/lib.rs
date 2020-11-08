use serde::{Deserialize, Serialize};

pub mod net;
pub use net::{messages::*, send_or_err, CLIENT, SERVER};

mod math;
pub use math::*;

#[macro_export]
macro_rules! or_err {
    ( $r:expr ) => {
        if let Err(e) = $r {
            log::error!("{}", e)
        }
    };
    ( $l:literal, $r:expr ) => {
        if let Err(e) = $r {
            log::error!($l, e)
        }
    };
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub enum Art {
    Island,
    Vase,
}
impl Art {
    pub fn size(self) -> f32 {
        match self {
            Self::Island => 1.0,
            Self::Vase => 1.0,
        }
    }
}
