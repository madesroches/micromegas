use crate::{
    cumulative_call_graph::span_overlaps,
    lakehouse::span_table::{SpanRow, TabularSpanTree},
};
use anyhow::Result;
use micromegas_tracing::span_fn;

pub type CallNodeHashMap = std::collections::HashMap<u32, CallGraphNode>;

#[derive(Debug)]
pub struct CallGraphNode {
    hash: u32,
    begin_ms: f64, //begin/end of the whole graph don't belong in each node
    end_ms: f64,
    sum: f64,
    sum_sqr: f64,
    min: f64,
    max: f64,
    count: u64,
    child_sum: f64,
    parents: CallNodeHashMap,
    children: CallNodeHashMap,
}

impl CallGraphNode {
    #[span_fn]
    pub fn new(hash: u32, begin_ms: f64, end_ms: f64) -> Self {
        Self {
            hash,
            sum: 0.0,
            sum_sqr: 0.0,
            begin_ms,
            end_ms,
            min: f64::MAX,
            max: f64::MIN,
            count: 0,
            child_sum: 0.0,
            parents: CallNodeHashMap::new(),
            children: CallNodeHashMap::new(),
        }
    }

    #[span_fn]
    pub fn add_call(&mut self, tree: &TabularSpanTree, span: &SpanRow) -> Result<()> {
        let time_ms = self.acc_stats(span);
        if span.parent != 0 {
            self.add_parent_call(tree.get_span(span.parent)?, time_ms);
        }
        if let Some(children) = tree.span_children.get(&span.id) {
            for childid in children {
                let child = tree.get_span(*childid)?;
                if span_overlaps(child, self.begin_ms, self.end_ms) {
                    self.add_child_call(child);
                }
            }
        }
        Ok(())
    }

    #[span_fn]
    fn acc_stats(&mut self, node: &SpanRow) -> f64 {
        let time_ms = node.end_ms.min(self.end_ms) - node.begin_ms.max(self.begin_ms);
        self.sum += time_ms;
        self.sum_sqr += time_ms.powf(2.0);
        self.min = self.min.min(time_ms);
        self.max = self.max.max(time_ms);
        self.count += 1;
        time_ms
    }

    #[span_fn]
    fn add_parent_call(&mut self, parent: &SpanRow, time_ms: f64) {
        let parent_node = self
            .parents
            .entry(parent.hash)
            .or_insert_with(|| Self::new(parent.hash, self.begin_ms, self.end_ms));
        parent_node.acc_stats(parent);
        parent_node.child_sum += time_ms;
    }

    #[span_fn]
    fn add_child_call(&mut self, child: &SpanRow) {
        let child_node = self
            .children
            .entry(child.hash)
            .or_insert_with(|| Self::new(child.hash, self.begin_ms, self.end_ms));
        child_node.acc_stats(child);
    }

    #[span_fn]
    pub fn to_proto_node(&self) -> CumulativeComputedCallGraphNode {
        CumulativeComputedCallGraphNode {
            stats: Some(self.get_proto_stats()),
            callees: self
                .children
                .iter()
                .map(|(_, node)| node.get_proto_stats())
                .collect(),
            callers: self
                .parents
                .iter()
                .map(|(_, node)| node.get_proto_stats())
                .collect(),
        }
    }

    #[span_fn]
    fn get_proto_stats(&self) -> CumulativeStats {
        CumulativeStats {
            hash: self.hash,
            count: self.count,
            max: self.max,
            min: self.min,
            sum: self.sum,
            sum_sqr: self.sum_sqr,
            child_sum: self.child_sum,
        }
    }
}
