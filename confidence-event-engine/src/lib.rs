pub mod event_logger;

pub mod proto {
    pub mod confidence {
        pub mod events {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/confidence.events.v1.rs"));
            }
        }
    }
}
