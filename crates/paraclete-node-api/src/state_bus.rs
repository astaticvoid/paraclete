use crate::node::Node;

/// A value published to the StateBus.
#[derive(Clone, Debug, PartialEq)]
pub enum StateBusValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Text(String),
}

/// A node that publishes named values to the StateBus.
///
/// Nodes implement `Node::published_state()` to provide values.
/// The runtime calls this after each audio cycle (on the executor thread)
/// and distributes the values to the StateBus snapshot.
///
/// Path conventions:
///   `/node/{id}/param/{name}`
///   `/node/{id}/state/{key}`
///   `/transport/bpm`  (for TempoSource nodes)
pub trait StatePublisher: Node {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_bus_value_is_clone() {
        let v = StateBusValue::Float(3.14);
        let w = v.clone();
        assert_eq!(v, w);
    }

    #[test]
    fn state_bus_value_variants_are_distinct() {
        assert_ne!(StateBusValue::Int(1), StateBusValue::Float(1.0));
        assert_ne!(StateBusValue::Bool(true), StateBusValue::Int(1));
    }
}
