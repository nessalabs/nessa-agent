//! Derived request-point feasibility; the controller owns admission decisions.
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    ExecutionController, MAX_EXECUTION_PERMISSIONS,
};
use std::mem::size_of;

#[derive(Clone, Copy, Default)]
struct Node {
    count: usize,
    bytes: usize,
    lazy_count: usize,
    lazy_bytes: usize,
    children: Option<(usize, usize)>,
}
#[derive(Default)]
pub(super) struct Intervals {
    nodes: Vec<Node>,
    points: usize,
}
pub(super) struct IntervalUndo {
    nodes: Vec<(usize, Node)>,
    len: usize,
    points: usize,
}
impl Intervals {
    pub(super) fn append(&mut self, bytes: usize) -> Result<(usize, IntervalUndo), AgentError> {
        let point = self.points;
        let undo = self.update(point, point + 1, bytes)?;
        self.points += 1;
        Ok((point, undo))
    }
    pub(super) fn retain_until_now(
        &mut self,
        point: usize,
        bytes: usize,
    ) -> Result<IntervalUndo, AgentError> {
        // At its own point the request already charged itself before release.
        self.update(point + 1, self.points, bytes)
    }
    fn update(
        &mut self,
        begin: usize,
        end: usize,
        bytes: usize,
    ) -> Result<IntervalUndo, AgentError> {
        let mut undo = IntervalUndo {
            nodes: Vec::new(),
            len: self.nodes.len(),
            points: self.points,
        };
        if self.nodes.is_empty() {
            self.nodes.push(Node::default());
        }
        if begin < end {
            self.add(
                0,
                0,
                MAX_EXECUTION_PERMISSIONS,
                (begin, end),
                bytes,
                &mut undo,
            );
        }
        let root = self.nodes[0];
        if let Err(error) = ExecutionController::validate_review_totals(root.count, root.bytes) {
            self.restore(undo);
            return Err(error);
        }
        Ok(undo)
    }
    fn add(
        &mut self,
        index: usize,
        low: usize,
        high: usize,
        range: (usize, usize),
        bytes: usize,
        undo: &mut IntervalUndo,
    ) {
        let (begin, end) = range;
        if begin >= high || end <= low {
            return;
        }
        undo.nodes.push((index, self.nodes[index]));
        if begin <= low && high <= end {
            let node = &mut self.nodes[index];
            node.count += 1;
            node.bytes = node.bytes.saturating_add(bytes);
            node.lazy_count += 1;
            node.lazy_bytes = node.lazy_bytes.saturating_add(bytes);
            return;
        }
        let (left, right) = match self.nodes[index].children {
            Some(children) => children,
            None => {
                let left = self.nodes.len();
                self.nodes.extend([Node::default(), Node::default()]);
                let children = (left, left + 1);
                self.nodes[index].children = Some(children);
                children
            }
        };
        let middle = low + (high - low) / 2;
        self.add(left, low, middle, range, bytes, undo);
        self.add(right, middle, high, range, bytes, undo);
        let l = self.nodes[left];
        let r = self.nodes[right];
        let node = &mut self.nodes[index];
        node.count = node.lazy_count + l.count.max(r.count);
        node.bytes = node.lazy_bytes.saturating_add(l.bytes.max(r.bytes));
    }
    pub(super) fn restore(&mut self, undo: IntervalUndo) {
        for (index, node) in undo.nodes.into_iter().rev() {
            self.nodes[index] = node;
        }
        self.nodes.truncate(undo.len);
        self.points = undo.points;
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.nodes.capacity().saturating_mul(size_of::<Node>())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_cancel_then_earlier_cancel_rejects_retroactive_overlap() {
        let mut state = Intervals::default();
        let (earlier, _) = state.append(17 * 1024 * 1024).unwrap();
        let (later, _) = state.append(17 * 1024 * 1024).unwrap();
        let undo = state.retain_until_now(later, 17 * 1024 * 1024).unwrap();
        assert!(state.retain_until_now(earlier, 17 * 1024 * 1024).is_err());
        state.restore(undo);
        assert!(state.retain_until_now(later, 17 * 1024 * 1024).is_ok());
    }
}
