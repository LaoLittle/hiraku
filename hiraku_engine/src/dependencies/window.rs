use hiraku_hdp::dependencies::{DependencyManifest, ResourceGraph};
use std::collections::{BTreeSet, VecDeque};

pub(super) fn locate(graph: &ResourceGraph, offset: usize) -> Option<usize> {
    // Innermost statement wins over enclosing if/while/closure expressions.
    graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.span[0] <= offset && offset < node.span[1])
        .min_by_key(|(_, node)| node.span[1] - node.span[0])
        .map(|(id, _)| id)
}

/// BFS gives both sides of a branch equal priority. Shared visits and limits
/// bound loops, recursion and branch fanout; no host function is executed.
pub(super) fn collect(
    manifest: &DependencyManifest,
    seeds: impl IntoIterator<Item = (String, usize)>,
    depth: usize,
    max_nodes: usize,
) -> Vec<String> {
    let mut queue: VecDeque<_> = seeds
        .into_iter()
        .map(|(path, node)| (path, node, 0))
        .collect();
    let mut visited = BTreeSet::new();
    let mut images = BTreeSet::new();
    let mut result = Vec::new();
    while let Some((path, id, distance)) = queue.pop_front() {
        if distance > depth || !visited.insert((path.clone(), id)) {
            continue;
        }
        if visited.len() > max_nodes {
            break;
        }
        let Some(graph) = manifest.windows.get(&path) else {
            continue;
        };
        let Some(node) = graph.nodes.get(id) else {
            continue;
        };
        for image in &node.images {
            if images.insert(image.clone()) {
                result.push(image.clone());
            }
        }
        for next in &node.next {
            queue.push_back((path.clone(), *next, distance + 1));
        }
        for function in &node.calls {
            let owner = if graph.functions.contains_key(function) {
                &path
            } else if let Some(owner) = manifest.exports.get(function) {
                owner
            } else {
                continue;
            };
            if let Some(entry) = manifest
                .windows
                .get(owner)
                .and_then(|g| g.functions.get(function))
            {
                queue.push_back((owner.clone(), *entry, distance + 1));
            }
        }
    }
    result
}
