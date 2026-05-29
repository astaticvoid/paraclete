/// Runtime graph representation.
///
/// Uses `petgraph::stable_graph::StableDiGraph` so that node indices remain
/// stable across insertions and removals (removed nodes leave "holes" rather
/// than shifting indices). Per ADR-005, the representation must support cyclic
/// graphs — petgraph's `is_cyclic_directed` and the cycle-aware toposort are
/// used at connection time so that loop-break nodes can be introduced at P9
/// without a graph rewrite.
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::Topo;

/// Uniquely identifies a node in the runtime graph.
/// Exposed as a stable handle; petgraph's `NodeIndex` is the backing type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub(crate) NodeIndex);

/// Metadata stored per node in the petgraph graph.
pub(crate) struct NodeMeta {
    /// The user-visible node ID (u32 passed to `NodeConfigurator::add_node`).
    pub user_id: u32,
}

/// Metadata stored per edge (port connection) in the graph.
pub(crate) struct EdgeMeta {
    /// Source port id on the upstream node. Used for audio buffer wiring at P2.
    #[allow(dead_code)]
    pub src_port: u32,
    /// Destination port id on the downstream node. Used for audio buffer wiring at P2.
    #[allow(dead_code)]
    pub dst_port: u32,
    /// Port type of the source port. Determines what data flows over this edge.
    /// Stored here so the executor can build event-routing tables without
    /// re-querying node port lists.
    pub src_port_type: paraclete_node_api::PortType,
}

/// The runtime's directed graph. Edges run from output port to input port
/// (data flow direction — upstream → downstream).
pub(crate) type RuntimeGraph = StableDiGraph<NodeMeta, EdgeMeta>;

/// Compute a linear execution order for the graph.
///
/// Performs a topological sort on the DAG subgraph. For P0 graphs (no cycles),
/// this is exact. Cycles are detected and returned as an error — the caller is
/// responsible for rejecting cyclic connections until loop-break nodes are
/// introduced at P9.
///
/// Returns `Ok(Vec<NodeIndex>)` in execution order (sources first, sinks last).
/// Returns `Err(Vec<NodeIndex>)` containing the nodes involved in at least one
/// cycle if the graph contains a cycle that does not pass through a loop-break
/// node.
pub(crate) fn execution_order(
    graph: &RuntimeGraph,
) -> Result<Vec<NodeIndex>, Vec<NodeIndex>> {
    // petgraph's Topo visitor performs a topological sort for DAGs.
    // It silently skips nodes involved in cycles — we detect cycles separately.
    if petgraph::algo::is_cyclic_directed(graph) {
        // Collect the nodes participating in cycles.
        // We use Tarjan's SCC algorithm: any SCC with more than one node
        // or a self-loop is a cycle.
        let sccs = petgraph::algo::tarjan_scc(graph);
        let cycle_nodes: Vec<NodeIndex> = sccs
            .into_iter()
            .filter(|scc| scc.len() > 1 || graph.contains_edge(scc[0], scc[0]))
            .flatten()
            .collect();
        return Err(cycle_nodes);
    }

    let mut topo = Topo::new(graph);
    let mut order = Vec::with_capacity(graph.node_count());
    while let Some(idx) = topo.next(graph) {
        order.push(idx);
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_node(graph: &mut RuntimeGraph, id: u32) -> NodeIndex {
        graph.add_node(NodeMeta { user_id: id })
    }

    fn connect(graph: &mut RuntimeGraph, src: NodeIndex, dst: NodeIndex) {
        graph.add_edge(src, dst, EdgeMeta {
            src_port: 0,
            dst_port: 0,
            src_port_type: paraclete_node_api::PortType::Audio,
        });
    }

    // ── execution_order ───────────────────────────────────────────────────────

    #[test]
    fn execution_order_single_node_returns_that_node() {
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let order = execution_order(&g).unwrap();
        assert_eq!(order, vec![a]);
    }

    #[test]
    fn execution_order_linear_chain_sources_before_sinks() {
        // A → B → C  should produce [A, B, C].
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let b = add_node(&mut g, 2);
        let c = add_node(&mut g, 3);
        connect(&mut g, a, b);
        connect(&mut g, b, c);

        let order = execution_order(&g).unwrap();
        assert_eq!(order.len(), 3);
        // A must come before B, B before C.
        let pos = |idx| order.iter().position(|&n| n == idx).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(b) < pos(c));
    }

    #[test]
    fn execution_order_diamond_both_branches_before_sink() {
        //   A
        //  / \
        // B   C
        //  \ /
        //   D
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let b = add_node(&mut g, 2);
        let c = add_node(&mut g, 3);
        let d = add_node(&mut g, 4);
        connect(&mut g, a, b);
        connect(&mut g, a, c);
        connect(&mut g, b, d);
        connect(&mut g, c, d);

        let order = execution_order(&g).unwrap();
        assert_eq!(order.len(), 4);
        let pos = |idx| order.iter().position(|&n| n == idx).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(a) < pos(c));
        assert!(pos(b) < pos(d));
        assert!(pos(c) < pos(d));
    }

    // ── cycle detection (ADR-005) ─────────────────────────────────────────────

    /// ADR-005: cycles must be detected and rejected from P0.
    /// Loop-break nodes (which make cycles legal) are a P9 deliverable.
    #[test]
    fn cycle_detection_two_node_cycle_returns_err() {
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let b = add_node(&mut g, 2);
        connect(&mut g, a, b);
        connect(&mut g, b, a); // creates a cycle

        assert!(execution_order(&g).is_err());
    }

    #[test]
    fn cycle_detection_self_loop_returns_err() {
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        connect(&mut g, a, a);

        assert!(execution_order(&g).is_err());
    }

    #[test]
    fn cycle_detection_three_node_cycle_returns_err() {
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let b = add_node(&mut g, 2);
        let c = add_node(&mut g, 3);
        connect(&mut g, a, b);
        connect(&mut g, b, c);
        connect(&mut g, c, a); // A → B → C → A

        assert!(execution_order(&g).is_err());
    }

    #[test]
    fn cycle_detection_error_contains_cycle_participants() {
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let b = add_node(&mut g, 2);
        connect(&mut g, a, b);
        connect(&mut g, b, a);

        let err = execution_order(&g).unwrap_err();
        assert!(err.contains(&a) || err.contains(&b));
    }

    #[test]
    fn acyclic_graph_does_not_trigger_cycle_error() {
        let mut g = RuntimeGraph::new();
        let a = add_node(&mut g, 1);
        let b = add_node(&mut g, 2);
        connect(&mut g, a, b);

        assert!(execution_order(&g).is_ok());
    }
}
