//! One module per rule (or rule family). Each is the single grep target for
//! its rule: the doc comment with the WHY, the detection (or the pointer to
//! where the CFG builder collects it), and the diagnostic wording.
//!
//! Infrastructure stays rule-free: `crate::cfg` builds the graph and collects
//! structural facts that fall out of its scope walk, `crate::dataflow` owns
//! the fixpoint framework, `crate::project` links files. A rule module
//! exposes `check(...)`, and `check_body`/`check_source` in the crate root is
//! just the list of those calls.

pub mod attributes;
pub mod calls;
pub mod cycles;
pub mod dead_store;
pub mod definite;
pub mod duplicate_key;
pub mod effects;
pub mod exports;
pub mod faint;
pub mod freeze;
pub mod globals;
pub mod members;
pub mod module_scope;
pub mod must_use;
pub mod naming;
pub mod narrowing;
pub mod requires;
pub mod retention;
pub mod roblox;
pub mod shadowing;
pub mod strings;
pub mod style;
pub mod suspicious;
pub mod unused;
