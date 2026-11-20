//! Flow-sensitive fact propagation over the CFG.
//!
//! One framework, five analyses. Getting this right is most of the project;
//! build it before the type checker.
//!
//! The framework is `Analysis` + `analyze`: an analysis supplies a lattice
//! fact per local (entry value, join, transfer over events); the framework
//! owns the worklist fixpoint and the replay pass that hands every event to
//! an observer with the state in force just before it. The instances live
//! with their rules (`rules::definite`, `rules::freeze`); narrowing and
//! yield tracking build on the same plumbing (branch-sensitive refinement
//! will extend the framework when narrowing lands, not before).
//!
//! `BackwardAnalysis` + `analyze_backward` is the reverse twin — facts flow
//! from exits toward the entry, joined over successors — for
//! liveness-shaped questions. First client: dead stores
//! (`rules::dead_store`, R0104).

use crate::cfg::{Cfg, Event};

/// A forward dataflow analysis: one lattice fact per local, transferred
/// across events in block order, joined where control flow merges.
pub trait Analysis {
    type Fact: Clone + PartialEq;
    /// The fact every local carries at function entry.
    fn entry_fact(&self) -> Self::Fact;
    /// Join at merge points. Which lattice this is (must vs may) is the
    /// analysis's soundness argument — document it at the impl.
    fn join(&self, a: &Self::Fact, b: &Self::Fact) -> Self::Fact;
    /// Apply one event to the state vector (indexed by `LocalId`).
    fn transfer(&self, state: &mut [Self::Fact], ev: &Event);
}

/// Worklist fixpoint: final in-state per block, `None` = unreachable.
fn fixpoint<A: Analysis>(cfg: &Cfg, analysis: &A) -> Vec<Option<Vec<A::Fact>>> {
    let mut input: Vec<Option<Vec<A::Fact>>> = vec![None; cfg.blocks.len()];
    input[cfg.entry] = Some(vec![analysis.entry_fact(); cfg.locals.len()]);

    let mut worklist = vec![cfg.entry];
    while let Some(b) = worklist.pop() {
        let mut out = input[b].clone().expect("worklist blocks have a state");
        for ev in &cfg.blocks[b].events {
            analysis.transfer(&mut out, ev);
        }
        for &succ in &cfg.blocks[b].succs {
            let changed = match &mut input[succ] {
                slot @ None => {
                    *slot = Some(out.clone());
                    true
                }
                Some(existing) => {
                    let mut changed = false;
                    for (e, o) in existing.iter_mut().zip(&out) {
                        let joined = analysis.join(e, o);
                        if joined != *e {
                            *e = joined;
                            changed = true;
                        }
                    }
                    changed
                }
            };
            if changed {
                worklist.push(succ);
            }
        }
    }
    input
}

/// A backward dataflow analysis: one lattice fact per local, transferred
/// across events in REVERSE order, joined over successors. The fact vector
/// describes what happens to each local on the paths AFTER the current
/// point — liveness-shaped questions.
pub trait BackwardAnalysis {
    type Fact: Clone + PartialEq;
    /// The fact every local carries at function exit (a `return` block or
    /// the fall-off block).
    fn exit_fact(&self) -> Self::Fact;
    /// Join where paths diverge (over a block's successors). Must vs may is
    /// the analysis's soundness argument — document it at the impl.
    fn join(&self, a: &Self::Fact, b: &Self::Fact) -> Self::Fact;
    /// Fold one event into the state, walking backward: on entry `state`
    /// describes what happens after `ev`; on exit, from `ev` onward.
    fn transfer(&self, state: &mut [Self::Fact], ev: &Event);
}

/// Backward worklist fixpoint: final out-state per block (the fact vector in
/// force at the block's END), `None` = no path from the block reaches an
/// exit was ever computed (unreachable backward).
fn fixpoint_backward<A: BackwardAnalysis>(cfg: &Cfg, analysis: &A) -> Vec<Option<Vec<A::Fact>>> {
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); cfg.blocks.len()];
    for (b, block) in cfg.blocks.iter().enumerate() {
        for &s in &block.succs {
            preds[s].push(b);
        }
    }

    let mut output: Vec<Option<Vec<A::Fact>>> = vec![None; cfg.blocks.len()];
    let mut worklist = Vec::new();
    for (b, block) in cfg.blocks.iter().enumerate() {
        // Exit blocks: `return <expr>` leaves the graph directly (no succs),
        // and fall_off is the shared no-succ sink for the rest.
        if block.succs.is_empty() {
            output[b] = Some(vec![analysis.exit_fact(); cfg.locals.len()]);
            worklist.push(b);
        }
    }

    while let Some(b) = worklist.pop() {
        let mut state = output[b].clone().expect("worklist blocks have a state");
        for ev in cfg.blocks[b].events.iter().rev() {
            analysis.transfer(&mut state, ev);
        }
        // `state` is now block b's IN-state; fold it into each predecessor's
        // out-state (a pred's end joins over everything its succs begin with).
        for &p in &preds[b] {
            let changed = match &mut output[p] {
                slot @ None => {
                    *slot = Some(state.clone());
                    true
                }
                Some(existing) => {
                    let mut changed = false;
                    for (e, s) in existing.iter_mut().zip(&state) {
                        let joined = analysis.join(e, s);
                        if joined != *e {
                            *e = joined;
                            changed = true;
                        }
                    }
                    changed
                }
            };
            if changed {
                worklist.push(p);
            }
        }
    }
    output
}

/// Backward fixpoint, then replay: every FORWARD-reachable block (findings
/// should be about code that executes; unreachable code is R0103's story)
/// is walked once more from its final out-state, in reverse, and `observe`
/// sees each event with the state describing what happens AFTER it.
pub fn analyze_backward<A: BackwardAnalysis>(
    cfg: &Cfg,
    analysis: &A,
    mut observe: impl FnMut(&[A::Fact], &Event),
) {
    let output = fixpoint_backward(cfg, analysis);
    let reachable = cfg.reachable();
    for (b, block) in cfg.blocks.iter().enumerate() {
        if !reachable[b] {
            continue;
        }
        let Some(state) = &output[b] else { continue };
        let mut state = state.clone();
        for ev in block.events.iter().rev() {
            observe(&state, ev);
            analysis.transfer(&mut state, ev);
        }
    }
}

/// Fixpoint, then replay: every reachable block is walked once more from its
/// final in-state, and `observe` sees each event with the state in force
/// just BEFORE that event applies. Findings are whatever the observer
/// collects.
pub fn analyze<A: Analysis>(
    cfg: &Cfg,
    analysis: &A,
    mut observe: impl FnMut(&[A::Fact], &Event),
) {
    let input = fixpoint(cfg, analysis);
    for (b, block) in cfg.blocks.iter().enumerate() {
        let Some(state) = &input[b] else { continue };
        let mut state = state.clone();
        for ev in &block.events {
            observe(&state, ev);
            analysis.transfer(&mut state, ev);
        }
    }
}
