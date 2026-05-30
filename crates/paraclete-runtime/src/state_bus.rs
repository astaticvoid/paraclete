use paraclete_node_api::StateBusValue;

/// One cycle's worth of state bus updates, pushed from the audio thread via SPSC.
pub struct StateBusUpdate {
    pub entries: Vec<(String, StateBusValue)>,
}
