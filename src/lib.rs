//! kritic: local, advisory, end-of-task governance for Claude Code.
//! Kev-4B (via laya) answers typed questions; deterministic checks and real project commands
//! supply facts. Deterministic approval is scoped to declared checks; visual craft assessments
//! are provisional and never confer aesthetic approval.

pub mod checks;
pub mod chunk;
pub mod cli;
pub mod commit_policy;
pub mod config;
pub mod context;
pub mod contract;
pub mod critic;
pub mod events;
pub mod files;
pub mod gpu;
pub mod history;
pub mod hygiene;
pub mod kev;
pub mod map;
pub mod metrics;
pub mod native;
pub mod organization;
pub mod phase3;
pub mod plugins;
pub mod proc;
pub mod provider;
pub mod repository;
pub mod scan;
pub mod scout;
pub mod server;
pub mod spec;
pub mod tools;
pub mod ui;
pub mod unity;
pub mod usage;
pub mod ux;
pub mod vision;
pub mod visual_quality;
